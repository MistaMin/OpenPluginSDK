# API review for the 1.0 freeze

This document lists the public items of every publishable crate and what was recommended for each before the API is frozen at 1.0.

**Status: applied.** The owner decided to apply every recommendation and, for each "decide" row, to take the way the recommendation points. The last column of each table says what was done, or why a row was not applied as written. The review was first checked against the tree as it is now; what was added since it was written is in [Items added since the review](#items-added-since-the-review). Every breaking change is in [../CHANGELOG.md](../CHANGELOG.md) with its migration, and the frozen surface is stated in [VERSIONING.md](VERSIONING.md#the-frozen-surface).

The text below the status is the review as it was written, at 0.3.1-beta: names such as `openplugin-ffi` and `FfiInstance` are the names of that time.

## How it was made

The list comes from rustdoc JSON (`cargo +nightly rustdoc -p <crate> --lib -- -Z unstable-options --output-format json`) on macOS at version 0.3.1-beta plus the 0.4 work in the tree, checked against the source (`grep -n "pub "`). Items behind `cfg(target_os = "macos")` are included (the `cocoa` module, `shell::editor`). Fields of structs and variants of enums are named in the row of their type. Methods and associated constants are named in the row of their type. Re-exports (`pub use`) are noted where the same item is reachable by two paths.

Size of the surface (modules, types, traits, functions, constants and methods; fields and variants not counted), and how many have no doc comment:

| Crate | Public items | Without docs |
|---|---|---|
| `openplugin-core` | 220 | 99 |
| `openplugin-dsp` | 188 | 85 |
| `openplugin-dri` | 72 | 40 |
| `openplugin-targets` | 164 | 118 |
| `openplugin-ffi` | 66 | 23 |
| `openplugin` (facade) | the `prelude` and five crate re-exports | n/a |

Roadmap task 3 (`missing_docs`) is where the undocumented items get docs. A freeze should wait for it, because writing the doc comment is when many of the doubts below show up.

## What was not applied as written

The rows that differ from their recommendation, each for a reason in today's tree:

| Row | What was done instead | Why |
|---|---|---|
| `Seqlock`, `spsc`: hide | Kept public and documented | PERFORMANCE.md, STABILITY.md and four porting guides, written after the review, teach plugins to pass meters and messages to their editor through them |
| `SURROUND_*`: hide | Kept | Since 0.6 they are the role ids `SpeakerLayout::roles`, `index_of` and `role_at` speak, with four ids CLAP does not have |
| `ParamInfo::flag` → `has_flag` | Renamed `with_flag` | It is a builder that adds a flag, not a query |
| One sample-rate type (F9) | The rule is written down, no signature changed | `f32` everywhere a plugin sees a rate; `f64` only where a host hands one over, so the range check runs before narrowing |
| `AaxIds`: validate on construction | `is_valid` became a `const fn` | Same shape as the later `Vst3Ids` and `AuIds`; a plugin can assert it at build time |
| `Editor`: only `size` and `render` required | `param_changed` stays required | An editor that ignores host values is a bug the compiler should catch |
| `DrawList::commands` generated on demand | Fields private, both lists kept | The shape methods write only the flat list and a lit knob's flat form is its classic drawing: neither list derives from the other |
| `controls`: a `Rect`/`Point` type | Positional arguments kept | It would put one more GoodLookin DRI type in the SDK's signatures (F11) |
| `Compressor::process_block` settings struct | `Compressor` deprecated | `Dynamics` supersedes it |
| `FfiInstance::store_param`/`load_param` naming | Kept, documented | They name the parameter plane (`ParamPlane::store`/`load`), not the plugin |
| `window::on_main_thread`: fix the non-macOS branch | Moved to `cocoa`; the other branch removed | Nothing called it |
| `embed_files` behind a feature | No feature | 64 lines, no dependency |
| Items superseded since the review (`Compressor`, `VuMeter`, `Oscillator`, `Wave`, `SampleVoice`, `Polyphony`, `Oversampler2x`, `window::hann`) | `#[deprecated]`, out of the prelude | They keep working through 1.x and go in 2.0; nothing in the SDK uses them outside their own tests |

## Recommendation codes

| Code | Meaning |
|---|---|
| **keep** | Freeze as it is. |
| **keep, mark** | Freeze, after a small change that does not alter use: `#[non_exhaustive]`, a doc fix, a missing `Default`/`Debug`. |
| **rename** | Right idea, wrong name or place. Change before the freeze. |
| **hide** | `#[doc(hidden)]`, or a non-default Cargo feature. The item must stay `pub` (macros, other crates in the workspace, tests) but is not promised. |
| **deprecate** | Keep working, mark `#[deprecated]` now, remove in a later major release. |
| **remove** | Delete before the freeze. |
| **decide** | A real design choice for the owner. The recommendation says which way and why. |

## Findings across the crates

These are the points that cut across modules. The per-crate tables refer to them by number.

**F1. Descriptor-only format layouts in `openplugin-targets`.** `vst3`, `au` and `aax` hold C layouts (`Vst3Guid`, `IComponentVtbl`, `AudioComponentDescription`, `AudioComponentPlugInInterface`, `AaxComponentDescriptor`) and three adapters (`Vst3Adapter`, `AuAdapter`, `AaxAdapter`) that wrap a plugin in a public field and do nothing else. Their own doc comments say a host cannot load them: the loadable shells are `native/vst3`, `native/au` and `native/aax`, over the `op_shell_*` ABI. They are leftovers from 0.1.0-beta.1. Nothing in the SDK uses them (the `shell` module does not). Publishing them at 1.0 would freeze types that look like the VST3/AU/AAX support and are not. Recommendation: remove the three modules and their re-exports, and the three prelude entries. The Cargo features `vst3`, `au`, `aax` and `clap` gate nothing (no `cfg(feature = ...)` exists in the crate), so they go too, or start gating something real. **Applied: the three modules, their adapters and the five no-op features are removed.**

**F2. `StandaloneRunner` does no audio.** `run_loop` calls `prepare` and logs; it opens no device and never calls `process`. The `standalone` feature pulls `cpal` as an optional dependency that no code uses. README and `FORMATS.md` describe Standalone as a CPAL driver and a window runner. Recommendation: remove it and the feature until a real standalone host exists (Windows/Linux roadmap), or at the least mark it `#[doc(hidden)]`. A public type that claims to run a plugin and does not is a worse freeze than no type. **Applied: removed, and README and FORMATS.md corrected. `cpal` had already gone.**

**F3. `DriView` and `DriHostWindow` are stubs.** `DriView::attach` matches a `RawWindowHandle` and logs; it draws nothing and holds no state beyond an empty `DrawList`. `DriView::render` returns that list. `DriHostWindow` is a two-method trait nothing implements in the SDK. The working editor path is `Editor` + `cocoa::EditorView` + `SceneCanvas`. `window::on_main_thread` is the only useful item in the module (and its non-macOS branch tests the thread *name*, which is wrong for plugin hosts). Recommendation: remove `DriView` and `DriHostWindow` and the `raw-window-handle` dependency from `openplugin-dri`; keep `on_main_thread` under a better home (`cocoa` or a `thread` module). **Applied: removed with `raw-window-handle`; `on_main_thread` is `cocoa::on_main_thread`.**

**F4. Raw ABI structs and bridge internals are `pub`.** `openplugin_targets::clap_abi` exposes about 60 `clap_*` `repr(C)` structs, about 70 constants and 4 statics; `lv2` exposes `Lv2Descriptor`, `Lv2Feature`, `Lv2UiDescriptor`; `openplugin_ffi::shell` exposes `OpShellInfo`, `OpParamDesc`, `ShellInstance` and every `op_shell_*` implementation as a Rust function (`create<P>`, `process`, `save_state`, ...); `shell::editor` exposes `OpEditorHost` and the editor entry functions; `clap::set_creator`, `set_editor`, `entry_get_factory`, `entry_deinit` and the `lv2` equivalents exist only for the export macros to reach. A plugin author never calls any of them. Freezing them freezes CLAP's layouts as the SDK's API (and a CLAP header bump would be breaking) and the Rust spelling of the C ABI, which is already covered by `OP_SHELL_ABI`. Recommendation: `#[doc(hidden)]` the macro support (`set_creator`, `set_editor`, `entry_*`, the whole of `clap_abi`, the `Lv2*` structs, `shell::{create, destroy, process, ...}`, `ShellInstance`, `OpShellInfo`, `OpParamDesc`, `OpEditorHost`), and state in VERSIONING that the `op_shell_*` / `op_editor_*` C ABI, not these Rust items, is the contract. `export_*!` macros stay the only supported entry. **Applied: `clap`, `clap_abi`, `shell` (both crates), `block` and the LV2 macro support are `#[doc(hidden)]`; VERSIONING names the C ABI and the format specifications as the contracts.**

**F5. Test harnesses and adapters presented as API.** `ClapAdapter`, `Lv2Harness` and `FfiInstance` are how tests and Rust hosts drive a plugin without a real host. They are useful, but they are three different shapes of the same job (`activate`/`run`/`process_*`), with different names for the same operations. Recommendation in each table; the common thread is to freeze `FfiInstance` as the Rust host API and demote the other two to `hide`. **Applied: `HostSession` is the Rust host API; `ClapAdapter` and `Lv2Harness` are hidden and out of the prelude.**

**F6. Two paths to the same item.** `openplugin-core` and `openplugin-dsp` re-export at the crate root what the modules define (`openplugin_core::AutomationLane` and `openplugin_core::automation::AutomationLane`), and the facade's `prelude` re-exports again. Every item is therefore reachable by two or three paths, and every path is frozen. Recommendation: keep the root re-exports as the documented paths; keep the modules `pub` only where they contain items that are not re-exported (`midi`, `spsc`, `preset`, `build`, `layout` constants); otherwise state in the module docs that the root path is the supported one. Do not move items after the freeze. **Applied as policy: VERSIONING freezes the root paths; the module paths stay reachable.**

**F7. Enums and structs that will grow, without `#[non_exhaustive]`.** None of the public enums and structs carries `#[non_exhaustive]`, so every new variant (a new `Category`, `Unit`, `SpeakerLayout`, `FilterType`, `SaturationMode`, `KnobStyle`, `Message`) is a breaking change under VERSIONING. Recommendation: mark `#[non_exhaustive]` on every enum the SDK will plausibly extend (listed per table), and on the `repr(Rust)` structs with public fields that plugins read but do not build. For `ParamInfo` and `PluginInfo`, which plugins do build, see the entries for `openplugin-core`. **Applied: see the tables, and the rule in VERSIONING (every public enum of `openplugin-dsp`; the listed ones elsewhere).**

**F8. Overlapping concepts.** The same job is done twice in several places; each is flagged in its table. The main ones: three metering paths on `Plugin` (`meters`, `bus_meters`, `gain_reduction_db`); `tail_samples` and `tail`; `note_on`/`note_off`/`midi`/`note_on_channel`/`pull_midi`; two preset mechanisms (`Plugin::preset_*` and `PresetBank`); two oversamplers and an oversample factor enum (`Oversampler2x`, `LinearPhaseOversampler`, `OversampleFactor`); five meter types (`VuMeter`, `MeterBank`, `BlockLevel`, `Loudness`, `MeterFrame`); two knob-style systems in `openplugin-dri` (the nine-variant `KnobStyle` enum and the 47 `draw_knob_code` strings); `DrawList::commands` and `DrawList::scene` as two public representations of one drawing; `ParamInfo::{clamp, to_normalized, from_normalized}` and `ParamSpec` in `openplugin-ffi` repeating `ParamInfo`'s fields. **Applied per table: `gain_reduction_db`, the style enums, `ParamSpec`, `OversampleFactor` are gone; `tail_samples` and the superseded DSP blocks are deprecated.**

**F9. Naming inconsistencies.** `Plugin::set_offline` against `FfiInstance::set_offline_flag`; `Plugin::param` (get by id) against `FfiInstance::load_param` / `store_param` (by index); `tail`/`tail_seconds`/`tail_samples`/`clap_samples`; `process_buses` against `process_with_sidechain`; `LaneMode` and `CurveInterpolation` as type aliases for `AutomationMode` and `AutomationInterpolation`; `KnobTier` (re-exported as an alias of `goodlookindri`'s `KnobMode`); `ParamInfo::default` (field) against `ParamSpec::default_value`; `PluginForExport` as an alias of `Plugin` in `openplugin-targets`; `Id` as the name of a raw pointer type in `cocoa`; sample rates are `f32` in `Plugin`/DSP and `f64` in `FfiInstance`/`sample_rate_supported` while `MIN_SAMPLE_RATE` is `f32`. **Applied, except the sample-rate type (a written rule, see above): `set_offline`, the two aliases, `PluginForExport`, `Id`, `default_value`, `clap_samples`.**

**F10. Product names in public docs.** The doc comments of `Seqlock`, `ParamPlane`, `rate` and `MeterBank` mention "Water Channel", a plugin outside this repository. Not an API change, but it reaches docs.rs. Replace with neutral wording. **Applied in the public module docs of core and dsp and in the LV2 module; the hidden `clap` modules keep their history notes.**

**F11. Dependencies in the public API.** `openplugin_dri` re-exports `goodlookindri` (needed: `examples/editor` builds `Scene`s through `openplugin::dri::goodlookindri`) and `goodlookindri_shell` (not used by any example or other crate). `KnobTier` is `goodlookindri`'s `KnobMode`. `DrawList` has public fields of `goodlookindri` types. So a breaking change in GoodLookin DRI is a breaking change of the SDK (VERSIONING says so). Roadmap task 2 makes it a versioned dependency; the review recommends keeping the `goodlookindri` re-export, dropping `goodlookindri_shell`, and wrapping nothing else. **Applied: `goodlookindri` stays re-exported with the version rule in its docs; `goodlookindri_shell` is no longer re-exported.**

**F12. Unused dependencies.** `log` is declared in `openplugin-core` and `openplugin-dsp` and not used there (`grep log::` finds nothing in either); `serde_json` in core is optional and nothing in `src` uses it; `cpal` and `raw-window-handle` in `openplugin-targets` are not used in its source (F2); in `openplugin-dri` `raw-window-handle` is used only by the stub `window` module (F3). This is roadmap task 1; listed here because removing a dependency that appears in a public derive is not free: `serde::Serialize` is implemented on `ParamInfo`, `PluginInfo`, `ParamFlags`, `Category`, `Unit` and `Taper`, and `Deserialize` on three of them, so `serde` stays a public dependency. **Applied: `log` and the five features leave `openplugin-targets`, `raw-window-handle` leaves `openplugin-dri` and the workspace. Core and dsp had been cleaned already.**

## Top ten recommendations

1. Remove the descriptor-only `vst3`, `au` and `aax` modules, their three adapters, and the no-op features (F1). **Applied.**
2. Remove `StandaloneRunner` and the `standalone` feature, or hide it (F2). **Applied: removed.**
3. Remove `DriView` and `DriHostWindow`, and `raw-window-handle` from `openplugin-dri` (F3). **Applied.**
4. Hide the macro and ABI internals of `openplugin-targets` and `openplugin-ffi`: `clap_abi`, `Lv2*` structs, `shell::*` functions, `ShellInstance`, `OpShellInfo`, `OpParamDesc`, `OpEditorHost`, `set_creator`, `set_editor`, `entry_*` (F4). **Applied.**
5. Decide the shape of `Plugin::process`: `&mut [Vec<f32>]` lets a plugin resize the host's buffers and forces an allocation-shaped type through the realtime path. A slice of slices (`&mut [&mut [f32]]`) is the usual form and is what `FfiInstance::process_planar` already takes. Changing it after 1.0 breaks every plugin. **Applied: `process(&mut self, io: &mut [&mut [f32]], frames: usize)` and `process_buses(io, sidechain: Option<&[&[f32]]>, frames)`. `ChannelsMut` and `Channels` borrow a host's `Vec<Vec<f32>>` as those slices without allocating.**
6. Add `#[non_exhaustive]` to the enums the SDK will extend (F7) and give `PluginInfo` and `ParamInfo` a const constructor path (`PluginInfo::new(..)` with `with_*` builders) so they can be `#[non_exhaustive]` too. `PluginInfo` is built by struct literal in the README and every example; `ParamInfo` gained two public fields in 0.1.1 already. **Applied.**
7. Collapse the overlapping `Plugin` methods (F8): deprecate `tail_samples` in favour of `tail`; fold `gain_reduction_db` into `meters`; keep one note path (`note_on_channel`/`midi`) and make `note_on`/`note_off` provided shims; move the factory-preset trio to the `PresetBank` mechanism. **Applied: `tail_samples` deprecated, `gain_reduction_db` removed; the note chain and the preset trio are documented rather than changed (see their rows).**
8. Choose one knob-style system in `openplugin-dri`: the nine-variant `KnobStyle` enum or the string codes of GoodLookin DRI (47 styles), and one drawing representation in `DrawList` (`scene`, with `commands` generated on demand) (F8). **Applied: the style codes; `DrawList`'s fields are private.**
9. Reduce `openplugin-dsp`'s duplicates: one oversampler pair, and decide whether the music-making toys (`Oscillator`, `Polyphony`, `SampleVoice`, `OnePole`-style helpers, `pitch`, `musical`, `pcm::lerp`) belong in a 1.0 plugin SDK or in a separate `-extras` crate (F8). **Applied by deprecation; no `-extras` crate.**
10. Fix naming and units before the freeze: one sample-rate type (`f32` or `f64`), `set_offline` vs `set_offline_flag`, the two type aliases `LaneMode`/`CurveInterpolation`, `PluginForExport`, and the product names in docs (F9, F10). Then run roadmap task 3 so every remaining item is documented. **Applied, with the sample-rate rule written down instead of a type change. `missing_docs` (roadmap task 3) is still open.**

## `openplugin-core`

Plugin traits and descriptors, the host plane, state, MIDI, layouts, the editor contract and presets. Depends on `serde` (public derives).

### Crate root

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `Plugin` (trait, about 40 methods) | keep, mark; decide | The centre of the SDK. The six required methods (`info`, `prepare`, `reset`, `set_param`, `param`, `process`) are fine to freeze; adding a required one later is breaking, so all additions must be provided methods. Open points: `process` signature (top ten 5), overlap in F8, `set_offline` naming, format-specific `aax_ids` and `lv2_uri` living on the core trait (acceptable; they have defaults). `save_state(&mut self)` and `load_state` take plain bytes and are the compatibility contract for plugin chunks: keep. | Applied. The trait docs name the six required methods, the call order and which note form to override. `process` takes `&mut [&mut [f32]]` (top ten 5). `aax_ids` and `lv2_uri` stay, beside the later `clap_id`, `vst3_ids` and `au_ids`. |
| `Plugin::tail_samples`, `Plugin::tail` | deprecate `tail_samples` | `tail` supersedes it and defaults from it. | Applied: `tail_samples` is `#[deprecated]`; `Tail::from_samples` converts a count; the SDK's own plugins override `tail`. |
| `Plugin::meters`, `bus_meters`, `gain_reduction_db` | decide | Three ways to report levels. `gain_reduction_db` is already inside `MeterFrame::reduction_db` and `BusMeters::reduction_db`. | Applied: `gain_reduction_db` is removed (no bridge read it); `MeterFrame::reduction_db` carries it. `meters` (stereo) and `bus_meters` (wider buses) stay. |
| `Plugin::note_on`, `note_off`, `midi`, `note_on_channel`, `pull_midi`, `all_notes_off` | decide | `note_on`/`note_off` have no channel; `note_on_channel` adds it; `midi` takes raw bytes. Keep `midi` + `note_on_channel` + `pull_midi` and demote the two channel-less calls to shims. | Applied as documentation, nothing removed: since the note-id work the bridges call `note_on_ref`/`note_off_ref`, which default to `note_on_channel` and `note_off`; the trait says which form to override. |
| `Plugin::preset_count`, `preset_name`, `preset_values` | keep, mark | They are what hosts read; fine, but have `PresetBank` produce them (F8). | Applied: the docs point to `PresetBank`, which answers all three. |
| `Plugin::set_param`, `set_param_index`, `param` | keep | String id for convenience, index for the audio thread. Document that the string forms may be slow. | Applied (docs). |
| `Plugin::prepare`, `prepare_layout`, `set_layout`, `supported_layouts`, `process_buses` | keep, mark | Layout calls are the right extension point. Document the call order once. | Applied: the call order is in the trait docs. |
| `Plugin::set_offline` | rename | `set_offline_flag` on `FfiInstance` differs (F9). Pick one. | Applied: `HostSession::set_offline`. |
| `Plugin::accepts_sample_rate` | keep | Default encodes `MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE`; documented. | Kept. |
| `Plugin::latency`, `set_transport`, `set_host_bypass`, `keeps_latency_when_bypassed`, `note_ports`, `editor_state`, `aax_ids`, `lv2_uri`, `info`, `reset` | keep | Provided methods with documented defaults. | Kept. |
| `PluginInfo` (10 fields, `param`) | decide | Built by struct literal everywhere. Add a const builder and `#[non_exhaustive]`, or accept that its 10 fields never grow. `serde::Serialize` only: keep or drop it (F12). | Applied: `PluginInfo::new` and `with_url`, `with_email`, `with_instrument`, `with_editor`, all `const`; `#[non_exhaustive]`. `Serialize` kept. |
| `ParamInfo` (10 fields; `new`, `with_taper`, `with_steps`, `with_choices`, `with_flags`, `flag`, `clamp`, `to_normalized`, `from_normalized`, `parse`, `format`) | keep, mark | Builders are the right API. `#[non_exhaustive]` once the fields are read-only in practice. The normalisation maths is a compatibility surface (VERSIONING). `flag` should be named `has_flag`. | Applied: `#[non_exhaustive]`. `flag` is renamed `with_flag`, not `has_flag`: it adds a flag, like the other builders; the query is `flags.contains`. |
| `Category`, `Unit` | keep, mark | `#[non_exhaustive]`. `Unit::Choice` / `Unit::Toggle` are formatting hints that also select behaviour (`format`, `parse`): document. | Applied. |
| `Taper` (`Linear`, `Log`) | keep, mark | `#[non_exhaustive]`: other tapers will be wanted (exponential, S-curve). | Applied. |
| `Transport` | keep, mark | Plain data, all fields public; `#[non_exhaustive]` so the host can add fields (loop points exist in CLAP already). | Applied: `#[non_exhaustive]`; a host starts from `Transport::default()`. |
| `MeterFrame` (9 fields, `SILENT`) | keep, mark | `#[non_exhaustive]`. Shares fields with `BusMeters` (F8). | Applied: `#[non_exhaustive]`; a plugin starts from `MeterFrame::SILENT`. |
| `AaxIds` (4 fields, `is_valid`) | keep, mark | Part of plug-in identity (VERSIONING). Validate on construction instead of `is_valid`, if it can be a const fn. | Applied in part: `is_valid` is a `const fn`, so a plugin can assert it at build time. It stays a check, not a constructor, as on the later `Vst3Ids` and `AuIds`. |
| `MIN_SAMPLE_RATE`, `MAX_SAMPLE_RATE` | keep | `f32`; see F9 for the `f64` mismatch. | Kept. |
| `sample_rate_supported` | keep, mark | Takes `f64` while the constants are `f32`: pick one type. | Not changed; the rule is now written on the function (F9): a rate is `f32` wherever a plugin or a DSP block sees one, and `f64` only where a host hands it over (`sample_rate_supported`, `HostSession::new`/`prepare`, the C ABI), because the range check must run before the value is narrowed. |
| `release_momentary` | keep | Documented, used by offline renders. Consider making it a `Plugin` provided method. | Kept. |

### `automation`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `AutomationLane` (new, lower, upper, mode, set_mode, interpolation, set_interpolation, hand, set_hand, heard, curve, touch, touching, set_rolling, rolling, is_rolling, insert_point, clear_points, set_playhead, playhead, points) | decide | 760 lines of host-side automation recording and playback with 18 integration tests, but no bridge, plugin or example in the SDK uses it. A plugin does not need it; a Rust host might. Hide behind a feature (`automation`), or move to a crate of its own, rather than freeze 21 methods nobody exercises through the SDK. | Applied: behind the Cargo feature `automation` (off by default), with the two enums. |
| `AutomationMode`, `AutomationInterpolation` | keep, mark | `#[non_exhaustive]` if `AutomationLane` stays. | Applied. |
| `LaneMode`, `CurveInterpolation` (type aliases) | remove | Aliases for the two enums above under older names (F9). | Applied: removed. |

### `build`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `embed_files` | keep | For a plugin's `build.rs`. Returns `io::Result`; documented. Consider a `build-helpers` feature so runtime binaries do not carry it. | Kept. No feature: it is 64 lines with no dependency. |

### `editor`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `Editor` (trait: size, bind, param_changed, tick, render, mouse_down, mouse_move, mouse_up, wheel) | keep, mark; decide | The editor contract. Resizing, keyboard and focus are planned for 0.5 (roadmap) and will add methods; every one must be provided. Keep `size`/`render` required only. Decide before the freeze whether `size` stays a fixed `(u32, u32)`, since 0.5 changes it to a range. | Kept. `size`, `param_changed` and `render` are required (an editor that ignores host values is a bug the compiler should catch); every method added since is provided. `size` is the initial size and `size_range` the range, as 0.5 decided. |
| `EditorFactory` | keep | `fn() -> Box<dyn Editor>`-style alias; the export macros take it. | Kept. |
| `EditorHost` (new, detached, send, begin, perform, end, set) | keep, mark | `new`/`detached` are for bridges and tests; `begin`/`perform`/`end`/`set` are what editors call. | Kept. |
| `Gesture` (Begin, Perform, End) | keep, mark | `#[non_exhaustive]` (a future `Cancel`). | Applied. |
| `Frame` (pixels, width, height) | keep, mark | Pixel format `0x00RRGGBB` is documented; changing it breaks every editor. `#[non_exhaustive]` and note the format in the type docs. | Applied: `#[non_exhaustive]`, built with `Frame::new`; the pixel format is in the type docs. |
| `EditorState` (alias for `Arc<dyn Any + Send + Sync>`) | keep | Shared engine/editor state; downcast by the plugin. | Kept. |
| `GestureQueue` (new, push, pop, is_empty), `ValueMirror` (new, invalidate, update) | hide | Only `clap.rs` uses them, to move values between threads; a plugin does not need them. Overlaps with `spsc` (F8). | Applied: `#[doc(hidden)]`. |

### `layout`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `SpeakerLayout` (12 variants; channels, kind, surround_map, surround_mask, label), `PortKind`, `IoConfig` (input, output, sidechain, `STEREO`, main_channels, input_channels, output_channels, sidechain_channels, name), `NotePorts`, `Tail` (None, Finite, Unknown, Infinite; clap_samples), `ParamFlags` (AUTOMATABLE, STEPPED, HIDDEN, READ_ONLY, BYPASS, MOMENTARY, PRESET_EXCLUDE, NOT_REALTIME; bits, contains, union, difference), `BusMeters` (channels, peak_db, reduction_db, `SILENT`) | keep, mark | `#[non_exhaustive]` on the enums; `ParamFlags` is a hand-written bit set whose bit values are part of the contract: document them or build it from a macro. `Tail::clap_samples` is CLAP-specific: rename `to_clap_samples` or hide. | Applied: `#[non_exhaustive]` on `SpeakerLayout`, `PortKind` and `Tail`; the bit of each `ParamFlags` flag is documented as part of the API; `Tail::clap_samples` is `to_clap_samples`; `Tail::from_samples` added. |
| `STEREO_LAYOUTS` (static), `MAX_AUDIO_CHANNELS`, `BUS_METER_CHANNELS` | keep | Constants. | Kept. |
| `SURROUND_FL`, `SURROUND_FR`, `SURROUND_FC`, `SURROUND_LFE`, `SURROUND_BL`, `SURROUND_BR`, `SURROUND_FLC`, `SURROUND_FRC`, `SURROUND_BC`, `SURROUND_SL`, `SURROUND_SR`, `SURROUND_TC`, `SURROUND_TFL`, `SURROUND_TFC`, `SURROUND_TFR`, `SURROUND_TBL`, `SURROUND_TBC`, `SURROUND_TBR` | hide | CLAP's surround channel codes. They are wire constants of the CLAP extension, not plugin API. | Not hidden. Since 0.6 these are the role ids of `SpeakerLayout::roles`, `index_of` and `role_at`, and the SDK defines four of its own (`SURROUND_TSL`, `TSR`, `FLW`, `FRW`): they are plugin API. |

### `midi`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `Message` (19 variants; write_bytes), `Parser` (new, running_status, pull), `U7` (try_new, get), `Channel` (try_new, index, number), `MidiError`, `PITCH_BEND_CENTER`, `deliver` | keep, mark | A MIDI 1.0 parser is useful to instrument and MIDI-effect authors. `#[non_exhaustive]` on `Message` (MIDI 2.0 will want more) and `MidiError`. `deliver` is the rule that maps a `Message` to `Plugin::note_on`/`note_off`/`midi` (no bridge or example in the SDK calls it); keep it, since a Rust host needs that rule. `Parser::pull` is the only iteration form: consider `Iterator`. | Applied: `#[non_exhaustive]` on `Message` and `MidiError`. `Parser::pull` stays the one form. |

### `plane`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `ParamPlane` (new, len, is_empty, infos, revision, store, store_id, load, load_id, seed_from), `ParamCursor` (new, invalidate, pull) | keep, mark | Lock-free parameter exchange between threads; a Rust host needs it and `Plugin::editor_state`-style code reads it. `store`/`load` by index, `*_id` by name: clear. | Kept; the module docs state the index and id forms. `ParamCursor::pulled`, `mark_pulled` and `pull_index` (added after the review, used by the CLAP bridge) are kept public: additive, and a Rust host applying one parameter needs the same. |

### `preset`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `Preset` (name, values), `PresetBank` (parse_all, len, is_empty, name, values, find) | keep, mark | The text preset format is a saved-state format (VERSIONING) with no version line: add one before freezing, or declare the format frozen as it is. | Applied: the format is declared frozen as it is, in the module docs and in VERSIONING. |
| `param_index`, `parse`, `write` | keep, mark | Errors are `String`: use a typed error. `parse` and `write` are the only way to author preset files; they are not re-exported at the root, which is fine. | Applied: `parse` returns `PresetError` (line and cause; `Display` gives the old text). `PresetBank::parse_all` still reports `file: error` strings. |

### `ramp`, `smooth`, `seqlock`, `spsc`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `SampleRamp` (new, value, set, begin, push, next) | rename | `next` shadows `Iterator::next` (a clippy allow exists for it): rename `next_value`. | Applied: `next_value`. |
| `OnePole` (new, set_time, reset, current, next) | keep, mark | Same `next` name question. | Applied: `next_value`. |
| `Seqlock` (new, publish, read) | hide | Generic lock-free primitive for bridges. Not part of the plugin-author surface. | Not hidden. PERFORMANCE.md, STABILITY.md and the porting guides, written after this review, teach plugins to pass meters to their editor through it. Kept and documented; it left the prelude. |
| `spsc::{channel, Producer (push), Consumer (pop)}` | hide | Same: bridge plumbing; the module name `spsc` is general enough that it invites use. | Not hidden, for the same reason: the documented way to pass messages between the audio thread and one other thread. |

### `state`

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `encode_params`, `decode_params`, `SNAPSHOT_MAGIC`, `SNAPSHOT_VERSION` | keep | Saved-state format OPST v1 (VERSIONING). | Kept. `SNAPSHOT_VERSION_KEYED` (version 2, by parameter id) was added since. |
| `encode_session`, `decode_session`, `Session` (values, chunk), `SESSION_MAGIC`, `SESSION_VERSION` | keep | OPCH v1. | Kept. |
| `restore_params` | keep | Applies a decoded snapshot. | Kept. |
| `AbCompare` (new, on_b, toggle) | keep | Holds two opaque state blobs and swaps them; independent of the plugin. | Kept. |

## `openplugin-dsp`

Realtime-safe DSP blocks. Depends on `openplugin-core`, `rustfft`, `serde` and `log` (the last is not used in the source).

| Module | Items | Rec. | Reason | Applied |
|---|---|---|---|---|
| `adaa` | `ResidualAdaa1` (new, reset, process_tanh, process_soft_clip), `Adaa2Taylor` (new, reset, process_hard_clip), `log_cosh` | keep, hide `log_cosh` | Named after the algorithm, not the job: `ResidualAdaa1::process_tanh` and `process_soft_clip` are two functions on one struct. `log_cosh` is a math helper. Comments cite an external wiki; fine. | Applied: `log_cosh` hidden. |
| `biquad` | `Biquad` (new, reset, configure, process_sample), `FilterType` (LowPass, HighPass, BandPass, Notch, Peaking, LowShelf, HighShelf) | keep, mark | `#[non_exhaustive]` on `FilterType` (all-pass is missing). | Applied. |
| `bypass` | `SoftBypass` (prepare, set_latency, latency, set_bypassed, snap, wet, reset, mix) | keep | Matches what the shells do. | Kept. |
| `convolve` | `BlockConvolver` (new, channels, reset, set_kernel, process) | keep | Documented limit of 1024 taps; decide whether the limit is frozen or made a parameter. | Kept; the 1024-tap limit is frozen (`LongConvolver` takes longer impulses). |
| `delay` | `DelayLine` (new, max_delay, reset, process_sample) | keep | | Kept. |
| `delta` | `Delta` (prepare, set_latency, latency, set_enabled, is_enabled, snap, amount, reset, process) | keep | | Kept. |
| `dynamics` | `Compressor` (new, set_sample_rate, reset, current_gain_reduction_db, process_block) | keep, mark | `process_block` has many arguments (clippy allow); take a settings struct. The module holds only one block although the description promises "dynamics (compressor/limiter/gate)" in README: bring README or API in line. | Deprecated: `Dynamics` (compressor, expander and gate, added since) supersedes it, so `process_block` is left as it is. README now matches the API. |
| `gain_comp` | `GainComp` (new, with_limit, limit_db, correction_db, reset, process) | keep | | Kept. |
| `linear_phase` | `LinearPhaseOversampler` (prepare, factor, latency_samples, reset, process), `OversampleError` | keep, one of two | Duplicates `Oversampler2x` (F8). Keep the linear-phase one as the oversampler and hide the 2x half-band, or the reverse; `OversampleError` is a four-variant enum: `#[non_exhaustive]`. | Kept as the oversampler; `OversampleError` is `#[non_exhaustive]`. |
| `oversampling` | `Oversampler2x` (new, set_sample_rate, reset, upsample_2x, downsample_2x), `OversampleFactor` (None, X2, X4) | decide | `OversampleFactor::None` is the only `None` that is a variant spelled that way (compare `Tail::None`): `X1`. Neither oversampler in this crate takes it (it is defined and re-exported, nothing more), so as a type it promises nothing: remove it or make `LinearPhaseOversampler::prepare` take it. | Applied: `OversampleFactor` is removed ("the type goes") and `Oversampler2x` is deprecated: `MinPhaseHalfband` (added since) is the 2x half-band, `LinearPhaseOversampler` the linear-phase one. Since then `Oversampler2x` is itself `MinPhaseHalfband` designed at 0.45 fs and 80 dB, so the replacement in the note is the same filter. |
| `loudness` | `KWeighting` (new, reset, process, coefficients), `Loudness` (new, reset, reset_integrated, push, momentary_lufs, short_term_lufs, integrated_lufs), `LoudnessMatch` (new, with_window, with_limit, with_gate, with_glide_ms, set_enabled, is_enabled, freeze, gain_db, target_db, reset, relearn, process), `SILENCE_LUFS`, `ABSOLUTE_GATE_LUFS`, `RELATIVE_GATE_LU`, `lufs_of`, `energy_of` | keep, hide helpers | BS.1770-4 is stable. `lufs_of`, `energy_of` are internal helpers. `KWeighting::coefficients` returns raw numbers: keep for tests, hide. | Applied: `lufs_of`, `energy_of` and `KWeighting::coefficients` hidden. |
| `meter` | `VuMeter` (new, set_sample_rate, reset, process_samples, level_db), `MeterBank` (new, push, readings, reset), `MeterReadings` (10 fields), `BlockLevel` (new, reset, push_stereo, short_db, integrated_db, true_peak_db), `correlation` | keep, mark | Five meter shapes across core and dsp (F8). `MeterBank` is documented as a plugin-specific bank ("the Water Channel meter bank"): neutral wording (F10). `MeterReadings` `#[non_exhaustive]`. | Applied: `VuMeter` deprecated (`VuIec` meets IEC 60268-17); `MeterReadings` `#[non_exhaustive]`; neutral wording. |
| `musical` | `samples_from_ms`, `ms_from_samples`, `ms_per_beat`, `samples_per_beat`, `samples_from_ticks` | keep | Small, pure. | Kept. |
| `oscillator` | `Oscillator` (new, phase, next), `Wave` (Sine, Saw, Square) | decide | A phase stepper (the module docs say so): `Saw` and `Square` are not band-limited and will alias. Either make them band-limited before freezing or keep them out of 1.0. | Deprecated, out of the prelude: `BandlimitedOsc` and `WavetableOsc` (added since) are the band-limited oscillators. |
| `pcm` | `i16_to_f32`, `f32_to_i16`, `i32_to_f32`, `f32_to_i32`, `lerp` | keep, move `lerp` | `lerp` is not PCM; put it with `musical` or a `math` module. | Applied: `lerp` lives in `utility`; the root path `openplugin_dsp::lerp` is unchanged. |
| `pitch` | `hz_from_midi`, `midi_from_hz` | keep | | Kept. |
| `rate` | `effective_rate`, `RatePlan` (host_hz, factor, internal_hz), `RateError`, `STUDIO_RATES_HZ`, `is_studio_rate`, `plan_rate` | keep, mark | `effective_rate` is the contract with the bridges' range. `plan_rate`'s doc refers to an outside plugin (F10); `RateError` `#[non_exhaustive]`; `u32` rates here against `f32` elsewhere (F9). | Applied: `RateError` `#[non_exhaustive]`, neutral wording. The `u32` rates stay: `plan_rate` multiplies whole rates. |
| `saturation` | `SaturationMode` (Tape, Tube, HardClip, SoftClip), `saturate_sample` | keep, mark | `#[non_exhaustive]`. Sits beside `adaa`, which does the band-limited versions of the same curves. | Applied. |
| `voice` | `SampleVoice` (new, is_active, start, stop, next), `Polyphony` (new, note_on, note_off, note_off_all, gain) | decide | Instrument helpers with a narrow shape (sample playback only). Same question as `oscillator`. | Deprecated, out of the prelude: `VoiceAllocator` and `SincPlayer` (added since) supersede them. |
| `window` | `hann`, `WindowRms` (new, reset, push) | keep | `hann` takes a phase, not a length; document. | `hann` deprecated (`WindowKind::Hann`); `WindowRms` kept. |

Root `openplugin_dsp` re-exports 50 of these by name; the facade's prelude re-exports 52. Keep the roots, per F6.

## `openplugin-dri`

The adapter between the SDK and GoodLookin DRI: drawing commands, controls, the scene canvas and the macOS view.

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `canvas::DrawList` (fields `commands`, `scene`, `knob_tier`, `floor`; new, clear, append, append_with_basic, fill_rect, stroke_rect, fill_rounded_rect, stroke_rounded_rect, fill_circle, stroke_circle, line, arc, text) | decide | Two parallel representations (`commands`, `scene`) are public fields; the 0.2 work made `scene` the real one and `commands` the fallback for hosts without images. Make the fields private with accessors, and generate `commands` on demand. `append_with_basic` (a lit knob's classic fallback) is plumbing for the knob drawing: hide. | Applied: the fields are private; `commands()`, `scene()`, `knob_tier()` and `set_knob_tier()` read and set them; `append_with_basic` hidden. Both lists are still kept, not generated: the shape methods (`fill_rect`, ...) write only the flat list, and a lit knob's flat form is its classic drawing, so neither list can be derived from the other. |
| `canvas::DrawCmd` (FillRect, StrokeRect, FillRoundedRect, StrokeRoundedRect, FillCircle, StrokeCircle, Line, Arc, Text) | keep, mark | `#[non_exhaustive]`; it is the fallback format. | Applied. |
| `controls::draw_faceplate`, `draw_faceplate_code`, `draw_knob`, `draw_knob_code`, `draw_vu_meter`, `draw_fader`, `draw_slider`, `draw_pan_knob`, `fader_pos_to_db`, `fader_db_to_pos`, `knob_angle` | keep, rename | Free functions with 5 to 9 positional arguments (clippy allows). Positions as `(x, y, w, h)` floats repeat. A `Rect`/`Point` type from `goodlookindri::scene` would remove the long lists. Fader mapping functions are plugin-facing (meters) and fine. `knob_angle` is a behaviour change from 0.3.1: freeze its sweep (±0.8 pi). | Applied: `draw_knob` and `draw_faceplate` are the code forms (the `_code` names and the enum forms are gone). The positional arguments stay: a `Rect` would put one more GoodLookin DRI type in the signatures (F11). |
| `styles::KnobStyle` (9), `FaceplateStyle` (6), `MeterStyle` (4), `Color` (r, g, b, a; rgba, rgb, WHITE, BLACK, VINTAGE_CREAM, BRIT_BLUE, BRIT_RED, BRIT_GREEN, BAKELITE_BROWN, VU_YELLOW) | decide | `KnobStyle` and `FaceplateStyle` are 9 and 6 of the 47 and 40 styles that GoodLookin DRI has; the `_code` functions reach all of them by string. Two systems (F8). Keep the strings (all styles, no enum to extend) and drop the enums, or generate the enums from GoodLookin's table. `Color` constants named after product looks (`BRIT_BLUE`) are a naming smell. | Applied: `KnobStyle` and `FaceplateStyle` removed, the style codes are the one system; `MeterStyle` `#[non_exhaustive]`; `Color::BRIT_BLUE`, `BRIT_RED`, `BRIT_GREEN` are `BLUE`, `RED`, `GREEN`. |
| `render::SceneCanvas` (new, invalidate_layer, set_background, size, frame, frame_layered, last_scene) | keep, mark | The way an editor gets pixels. `last_scene` is a debugging accessor: hide. Holds a process-wide paint lock (documented in the 0.3.1 changelog): document in the type docs, because it is a behaviour a plugin author can hit. | Applied: `last_scene` hidden; the paint lock is in the type docs. |
| `cocoa::EditorView` (new, raw, size, attach, set_hidden, editor, set_poll; Drop), `cocoa::Poll` | keep, mark | macOS only. The view is how every bridge hosts the editor. `raw()` returns an `Id`; resizing (0.5) will add methods. | Kept. The views' test hooks (menu and file hooks, `text_field`, `tooltip_rects`, ...) are hidden on all three platforms. |
| `cocoa::Id` (alias for `*mut c_void`) | rename | A public alias with the name of an Objective-C concept for a raw pointer. `NSViewPtr`, or return `NonNull<c_void>`. | Applied: `NSViewPtr`. |
| `cocoa::run_window` | keep, mark | Opens a window and runs the app; for the example's snapshot and manual testing. Main thread only; document. | Kept; main thread is in its docs. |
| `window::DriView`, `window::DriHostWindow` | remove | Stubs (F3). | Applied: removed, with `raw-window-handle`. |
| `window::on_main_thread` | keep, move | Useful, wrong location; the non-macOS fallback is incorrect. | Applied: `cocoa::on_main_thread`. The non-macOS branch is gone, not fixed: nothing called it. |
| `pub use goodlookindri` | keep | Needed to build scenes (F11). | Kept, with the version rule in its docs. |
| `pub use goodlookindri_shell` | remove | Used by `SceneCanvas` internally only. | Applied: removed. |
| `pub use goodlookindri::draw::KnobMode as KnobTier` | keep, mark | Foreign type under a local name: write the alias in the docs; a new GoodLookin tier shows up as an SDK change. | Applied (docs). |

## `openplugin-targets`

CLAP and LV2 entries, the shell macros, and leftovers.

| Module / item | Rec. | Reason | Applied |
|---|---|---|---|
| `export_clap!`, `export_lv2!`, `export_shell!` (macros) | keep | The supported way in. Their expansions refer to `$crate::...` paths: those paths are what F4 hides and must stay reachable while hidden. Document the macro argument forms once (`editor = make_editor`). | Kept; the crate docs give the two forms of each macro. |
| `clap::ClapAdapter` (new, as_plugin, meters, bus_meters, editor_state, plane, editor_host, activate; Drop) | hide | Test and macro use (F5). A plugin calls the macro. | Applied: hidden. |
| `clap::set_creator`, `set_editor`, `entry_get_factory`, `entry_deinit` | hide | Macro support (F4). `entry_get_factory` and `entry_deinit` are `unsafe extern "C"` entry points the macro puts in `clap_entry`. | Applied: the `clap` module is hidden. |
| `clap_abi` (the `clap_*` structs; `CLAP_VERSION`, `NAME_SIZE`, `PATH_SIZE`, `PROCESS_*`, `AUDIO_PORT_IS_MAIN`, `PARAM_IS_*`, `CLAP_INVALID_ID`, `NOTE_DIALECT_*`, `TRANSPORT_*`, `AMBISONIC_*`, `EVENT_*`, `EXT_*`, `RENDER_*`, `FACTORY_ID`, `WINDOW_API_COCOA`, `CLAP_PARAM_RESCAN_VALUES`; statics `PORT_MONO`, `PORT_STEREO`, `PORT_SURROUND`, `PORT_AMBISONIC`; structs `clap_version`, `clap_plugin_descriptor`, `clap_host`, `clap_host_params`, `clap_event_header`, `clap_event_param_value`, `clap_event_note`, `clap_event_midi`, `clap_input_events`, `clap_output_events`, `clap_audio_buffer`, `clap_process`, `clap_plugin`, `clap_audio_port_info`, `clap_plugin_audio_ports`, `clap_note_port_info`, `clap_plugin_note_ports`, `clap_plugin_latency`, `clap_param_info`, `clap_plugin_params`, `clap_istream`, `clap_ostream`, `clap_plugin_state`, `clap_plugin_factory`, `clap_plugin_entry`, `clap_plugin_tail`, `clap_plugin_render`, `clap_host_latency`, `clap_host_tail`, `clap_audio_ports_config`, `clap_plugin_audio_ports_config`, `clap_plugin_audio_ports_config_info`, `clap_plugin_surround`, `clap_ambisonic_config`, `clap_plugin_ambisonic`, `clap_event_transport`, `clap_event_param_gesture`, `clap_window`, `clap_gui_resize_hints`, `clap_plugin_gui`) | hide | A hand-written subset of the CLAP header, 118 undocumented items (F4). The macro needs `clap_plugin_entry` and its field types to be reachable. Hiding keeps the macro working and keeps CLAP's layout out of the SDK's promise. If the SDK later uses a published `clap-sys` crate, this module disappears. Also the lower-case type names break Rust naming (`#![allow(non_camel_case_types)]`), another reason to not publish them. | Applied: hidden. |
| `lv2::plugin_uri`, `binary_file`, `symbol`, `manifest_ttl`, `plugin_ttl`, `write_bundle` | keep | The `.ttl` writer is plugin-facing (`examples/editor`'s `lv2_ttl` binary uses it). Document the port layout (ports 0 and 1 in, 2 and 3 out, controls after). `symbol` is an implementation detail: hide. | Applied: `symbol` hidden; the port layout is in the module docs and FORMATS.md. |
| `lv2::AUDIO_PORTS` | keep | Constant of the port layout; document that it is 4. | Kept. |
| `lv2::set_creator`, `set_editor`, `descriptor`, `lv2ui_descriptor`, `Lv2Descriptor`, `Lv2Feature`, `Lv2UiDescriptor` | hide | Macro support and raw LV2 structs (F4). | Applied: hidden. |
| `lv2::Lv2Harness` (new, plane, meters, plugin, input, output, run) | hide | A test harness (F5). Public from the crate root and the prelude today. | Applied: hidden, out of the prelude. |
| `shell` re-exports from `openplugin_ffi::shell`: `create`, `destroy`, `editor_state`, `get_normalized`, `get_plain`, `latency`, `load_state`, `process`, `reset`, `save_state`, `set_normalized`, `set_plain`, `tail_seconds`, `to_normalized`, `to_plain`, `trace`, `write_info`, `write_param`, `OpParamDesc`, `OpShellInfo`, `ShellInstance`, `ABI`, `AAX_MAX_PARAMS`, `PARAM_AUTOMATABLE`, `PARAM_DISCRETE`, `PARAM_LOG`, `STATE_MAX`, `STATE_OK`, `STATE_TOO_SMALL` | hide | Macro support. The C ABI is the contract. `ABI` and the `STATE_*` / `PARAM_*` constants could stay documented as mirrors of `op_shell.h`. `AAX_MAX_PARAMS` is no longer used by the AAX shell (SHELL.md says so): remove. | Applied: the `shell` module is hidden; `AAX_MAX_PARAMS` removed. |
| `shell::editor` (module; `OpEditorHost` (context, begin_edit, perform_edit, end_edit), `host_for`, `size`, `create`, `raw_view`, `attach`, `bind`, `set_plain`, `destroy`), macOS | hide | Rust-side bodies of the `op_editor_*` functions; same as above. | Applied: hidden with `shell`. |
| `vst3` (`Vst3Guid`, `IComponentVtbl`, `Vst3Adapter`), `au` (`AudioComponentDescription`, `AudioComponentPlugInInterface`, `AuAdapter`), `aax` (`AaxComponentDescriptor`, `AaxAdapter` with `describe_effect`) | remove | F1. `AaxAdapter::describe_effect` returns pointers into `&'static str` that are not NUL-terminated, so the result cannot be used as C strings: a latent bug in an unused API. | Applied: removed. |
| `standalone::StandaloneRunner` (plugin, sample_rate, channels; new, run_loop) | remove | F2. | Applied: removed. A standalone build is the plugin's own binary calling `run_window`; README and FORMATS.md say so. |
| `PluginForExport` (alias for `Plugin`) | remove | The macros can name `openplugin_core::Plugin`. An alias of a core trait under a second name has no use. | Applied: removed; the macros name the core crate through a hidden re-export. |
| Cargo features `clap`, `vst3`, `au`, `aax`, `standalone` (default on) | remove or make real | Nothing is gated (F1, F2). | Applied: all five removed. |

## `openplugin-ffi`

The Rust host session and the Rust spelling of the `op_shell_*` ABI.

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `FfiInstance` (new, editor_state, tail_seconds, name, param_count, param_spec, preset_count, preset_name, store_param, load_param, store_normalized, load_normalized, latency, meters, bus_meters, preset_index, apply_preset, set_offline_flag, prepare, reset, save_bytes, try_save_bytes, load_bytes, process_stereo, process_io, process_planar, process_interleaved, process_with_sidechain) | keep, rename | The one Rust host API; freeze it. Names: `store_param`/`load_param` (by index, plain value) against `Plugin::set_param`/`param`; `set_offline_flag`; `save_bytes` returns an empty vector when the plugin object cannot be reached and `try_save_bytes` returns `None` (an empty vector is never a valid session, per its docs): keep one, or make both return `Result`. Five `process_*` forms for one operation: keep `process_planar` (and `process_with_sidechain`), turn `process_stereo`, `process_io`, `process_interleaved` into documented convenience wrappers or remove `process_io`. `new` cannot fail and silently replaces a bad rate with 48 kHz while `prepare` rejects it (documented): return a `Result`. | Applied. The type is `HostSession` (the crate is `openplugin-host`). `new` returns `Result<_, UnsupportedRate>`; `set_offline`; one `save_bytes` returning `Option`; `process_io` removed, `process_stereo` and `process_interleaved` documented as forms of `process_planar`. `store_param`/`load_param` stay: they name the parameter plane (`ParamPlane::store`/`load`), and the docs say so. |
| `ParamSpec` (id, name, min, max, default_value, is_discrete, steps, is_log, unit, flags), `spec_from` | decide | A flattened copy of `ParamInfo` for C-style hosts; nothing in the SDK uses it but `FfiInstance::param_spec`. Fields duplicate `ParamInfo` with different names (`default_value` vs `default`, `is_log` vs `Taper`). Replace by returning `&ParamInfo`. Remove `spec_from`. | Applied: both removed; `param_info` returns `&ParamInfo`. |
| `STATE_WAIT` | keep, mark | The two-second bound on state calls, documented in SHELL.md; a `const` of this kind is easy to want to change: freeze as a documented default, or make it a setter. | Kept as a documented constant. |
| `pub use openplugin_core::Plugin` | keep | Convenience. | Kept. |
| `shell` module: `create<P>`, `destroy`, `reset`, `latency`, `set_plain`, `get_plain`, `set_normalized`, `get_normalized`, `to_plain`, `to_normalized`, `process`, `save_state`, `load_state`, `write_info`, `write_param`, `tail_seconds`, `editor_state`, `ShellInstance`, `OpShellInfo`, `OpParamDesc`, `trace::{stem, on, write}`, `ABI`, `PARAM_LOG`, `PARAM_DISCRETE`, `PARAM_AUTOMATABLE`, `STATE_OK`, `STATE_TOO_SMALL`, `STATE_MAX`, `AAX_MAX_PARAMS` | hide | The Rust side of the C ABI (F4). `write_info`/`write_param` are already `unsafe` since 0.3.1. `trace` writes a file when an environment switch is set: a debugging aid, hide. `AAX_MAX_PARAMS` is stale: remove. | Applied: `shell` and `block` hidden; `AAX_MAX_PARAMS` removed. |
| Crate type | decide | `openplugin-ffi` is `rlib` only, but its name says FFI and the C surface is in `openplugin-targets`'s macro output. Rename to `openplugin-host` (it is "the Rust host session") before the name is published to crates.io, or document why not. | Applied: `openplugin-host`, `openplugin::host`. The old name is not kept. |

## `openplugin` (facade)

| Item | Rec. | Reason | Applied |
|---|---|---|---|
| `core`, `dsp`, `dri`, `targets`, `ffi` (re-exported crates) | keep | Lets a plugin depend on one crate. | Kept; `ffi` is `host`. |
| `prelude` (about 100 names) | keep, mark | Today it re-exports `Lv2Harness`, `StandaloneRunner`, `Vst3Adapter`, `AuAdapter`, `AaxAdapter`, `ClapAdapter`, and `Adaa2Taylor`, `plan_rate`, `lerp` and many DSP internals. A prelude should hold what nearly every plugin imports: `Plugin`, `PluginInfo`, `ParamInfo`, `Unit`, `Taper`, `Category`, `Editor` items, `Tail`, `IoConfig`, and the macros. Move the rest out. Removing names from a prelude is breaking, so do it before the freeze. | Applied: 20 names (the plugin and editor contracts, the descriptors, the channel adapters) and the three export macros. |

## Items added since the review

The review was written at 0.3.1-beta. These are the public items added between then and the freeze, each with a decision in the same spirit: plugin-facing items are kept, enums that will grow are `#[non_exhaustive]`, and what exists only for the bridges, the macros or the tests is hidden.

### `openplugin-core`

| Item | Decision | Applied |
|---|---|---|
| `channels::{ChannelsMut, Channels}` | keep | Added with this change: the channel slices `process` takes, over buffers a host owns, without allocation. |
| `Plugin`: `note_expressions`, `mpe_input`, `note_on_ref`, `note_off_ref`, `note_expression`, `param_modulation`, `voice_info`, `pull_note_end`, `flush_denormals`, `track_info_changed`, `set_host_notifier`, `clap_id`, `vst3_ids`, `au_ids`, `state_is_parameters_only` | keep | All provided methods whose default is the behaviour before they existed. |
| `ParamInfo`: `group`, `pins`, `with_group`, `with_host_id`, `with_clap_id`, `with_vst3_id`, `with_au_id`, `with_aax_id`, `host_group`, `group_names`; `check_group`, `groups_valid`, `group_paths`, `group_parent`, `group_name`, `GroupError`, `GROUP_SEPARATOR`, `GROUP_MAX_BYTES` | keep, mark | `GroupError` is `#[non_exhaustive]`. The group rules are what hosts are shown: frozen. |
| `TrackInfo` (name, colour, kind; `IS_RETURN`, `IS_BUS`, `IS_MASTER`, `is_return`, `is_bus`, `is_master`, `rgb`) | keep, mark | `#[non_exhaustive]`: a host adapter starts from `TrackInfo::default()`. |
| `identity`: `ParamPins`, `ParamIdMap`, `Vst3Ids`, `AuIds`, `PinError`, `PinFormat`, `check_pins`, `clap_id_valid`, `param_host_ids`, `param_aax_ids`, `params_pinned`, `state_is_keyed`, the reserved-id constants | keep, mark | `PinError` and `PinFormat` are `#[non_exhaustive]`. How a pin becomes what a host sees is identity (VERSIONING section 4). |
| `expression`: `Expression`, `ExpressionSet`, `NoteRef`, `VoiceInfo` | keep | `Expression` was `#[non_exhaustive]` already. |
| `mpe`: `MpeReceiver`, `MpeZones`, `MpeEvent`, `Zone`, `ZoneChannel`, `deliver`, `receive_into`, `receive_note_into`, `bend_unit`, the bend-range constants | keep, mark | `MpeEvent` is `#[non_exhaustive]`. `Zone` is the two zones of the MPE specification and stays exhaustive. |
| `notify::HostNotifier` | keep | |
| `worker`: `loader`, `with_executor`, `Loader`, `Slot`, `Task`, `Step`, `Executor`, `Waker`, `QueueFull` | keep | `Step` is what a plugin's task returns; it stays exhaustive. |
| `denormal::DenormalGuard` | keep | |
| `foreign_state` (`Reader`, `Endian`, `LenWidth`; `chunk`, `kv`, `plist`, `xml`, `import`; `ForeignError`, `ErrorKind`, `Limits`) | keep, mark | For reading another SDK's saved state in a port. `ErrorKind`, `ForeignId`, `ValueKind`, `plist::Value` and `xml::Node` are `#[non_exhaustive]`. |
| `editor`: `SizeRange`, `DamageRect`, `DamageTracker`, `Modifiers`, `Key`, `AccessNode`, `AccessRange`, `AccessRole`, `AccessAction`, `TextEdit`, `Tooltip`, `FileDialog`, `ParamMenu`, `ParamMenuAction`; the `Editor` methods for size, damage, keys, focus, accessibility, the parameter menu, text entry, tooltips, drops and file dialogs; `EditorHost::with_resize`, `with_context_menu`, `context_menu`, `request_resize` | keep, mark | `Key`, `AccessRole`, `AccessAction` and `ParamMenuAction` are `#[non_exhaustive]`. The structs an editor builds (`AccessNode`, `Tooltip`, `TextEdit`, `FileDialog`, `ParamMenu`) stay plain. |
| `layout`: the Atmos layouts and `SpeakerLayout::roles`, `index_of`, `role_at`, `is_lfe`, `non_lfe`, `pairs`, `unpaired`, `clap_expressible`, `clap_map`; `partner_role`, `is_left_role`, `CLAP_SURROUND_MAX`, `SURROUND_ROLE_COUNT`; `ParamFlags::MODULATABLE`, `POLY_MODULATION` | keep | |
| `state::SNAPSHOT_VERSION_KEYED` | keep | OPST version 2 (VERSIONING section 3). |
| `preset::PresetError` | keep | Added with this change. `#[non_exhaustive]`. |

### `openplugin-dsp`

Every module below was added after the review. All are kept. Every public enum of the crate is `#[non_exhaustive]`, so a mode, a kind or an error can be added in a minor release; the structs a plugin fills in (`DynamicsParams`, `RepairParams`, `SpectrumConfig`, ...) stay plain.

| Modules | Decision | Applied |
|---|---|---|
| Dynamics: `envelope`, `gain_computer`, `limiter`, `true_peak`, `clipper`, `deesser`, `dynamic_eq`, `multiband`, `transient` | keep, mark | `Dynamics` supersedes `Compressor`. |
| Filters: `svf`, `matched`, `filters`, `crossover`, `fir_design`, `fir_phase`, `halfband_min`, `interp` | keep, mark | `MinPhaseHalfband` supersedes `Oversampler2x`. |
| Meters: `kmeter`, `ppm`, `true_peak_meter`, `scope`, `spectrum`; `meter::VuIec`; `loudness::{EbuMeter, EbuScale, Target, bs1770_weight}` | keep, mark | `VuIec` supersedes `VuMeter`. |
| Time and modulation: `moddelay`, `lfo`, `chorus`, `flanger`, `phaser`, `delay_pitch`, `vocoder`, `pitch_detect`, `stft`, `windows` | keep, mark | `WindowKind::Hann` supersedes `window::hann`. |
| Reverb and convolution: `fdn`, `plate`, `early`, `partitioned` | keep, mark | |
| Virtual analog and physical models: `ladder`, `korg35`, `wdf`, `tape`, `pluck`, `waveguide`, `modal`, `neural` | keep, mark | `neural::MAX_LAYERS` (the loader's cap, added after the review) is kept: raising it is not a break. |
| Synthesis: `blosc`, `wavetable`, `adsr`, `noise`, `resample`, `voice_alloc`; `voice::{NoteExpressions, MOD_SLOTS}` | keep, mark | `BandlimitedOsc`, `WavetableOsc`, `VoiceAllocator` and `SincPlayer` supersede `Oscillator`, `Polyphony` and `SampleVoice`. `VoiceAllocator::ended_dropped` (a counter, added after the review) is kept. |
| Surround and utilities: `downmix`, `surround_adapter`, `dither`, `spectral_repair`, `utility`, `simd` | keep, mark | `SurroundAdapter` follows `Plugin::process`; its `meters` carries the deepest gain reduction of its engines, which `gain_reduction_db` gave before. |

### `openplugin-dri`

| Item | Decision | Applied |
|---|---|---|
| `design_editor`: `DesignEditor`, `DesignOptions`, `DesignEditorError`, `Binding`, `RowIssue`, `IssueKind`, `Look` | keep, mark | The three enums are `#[non_exhaustive]`. |
| `hit`: `ControlSet`, `Control`, `ControlKind`, `ControlId`, `Area`, `DoubleClick`, the step constants | keep, mark | `ControlKind` and `DoubleClick` are `#[non_exhaustive]`. `Area` (a rectangle or a circle) stays exhaustive. |
| `SceneCanvas::resize`, `frame_damaged` | keep | |
| `win32`: `EditorView`, `TopWindow`, `Hwnd`, `Poll`, `run_window`, `set_process_dpi_aware`; `win32_uia` | keep | Windows. |
| `x11`: `EditorView` (with `fd`, `pump`, `scale`), `Window`, `Poll`, `TICK_INTERVAL_MS`, `DOUBLE_CLICK_MS`, `CLIPBOARD_WAIT_MS`, `TOOLTIP_DELAY_MS`, `run_window` | keep | Linux. |
| The views' test hooks on all three platforms: `PARAM_MENU_ITEMS`, `MenuHook`, `FileHook`, `set_menu_hook`, `choose_menu_item`, `set_file_dialog_hook`, `text_field`, `tooltip_rects`; `cocoa::{FieldKey, field_key, key_monitor_count, open_panel_for, simulate_panel_response}`; `win32::{tooltip_control_texts, testing}`; `x11` `menu_window`, `text_field_text` and the clipboard calls | hide | `#[doc(hidden)]`: the SDK's own tests drive a view through them. |

### `openplugin-targets`

| Item | Decision | Applied |
|---|---|---|
| `lv2::PortMap`, `plugin_ttl_sized`, `write_bundle_for_editor` | keep | The port layout is part of the plugin's LV2 identity (VERSIONING section 4). |
| `lv2::layout_of`, `instance_state`, `worker_stats`, `WorkerStats` | hide | Bridge and test support. |
| `shell`: the ABI 4 to 8 additions (layouts, events, MIDI out, changes, expression, identity, track info) and `shell::editor`'s resize, context-menu and track calls | hide | With the module (F4). `OP_SHELL_ABI` 8 and `op_editor.h` are the contract. |
| The CLAP and LV2 editor bridges for X11 and Win32 (`clap_gui_x11`, `clap_gui_win32`, `lv2_ui_x11`, `lv2_ui_win32`) | hide | Private modules of the hidden bridges. |

### `openplugin-host` (was `openplugin-ffi`)

| Item | Decision | Applied |
|---|---|---|
| `HostSession::take_changes`, `set_track_info`, `select_layout`, `layout_index`, `layout`, `take_midi_dropped` | keep | |
| `HostSession::process_block`, `pull_midi_out` | hide | They take the C ABI's own structs (`OpProcess`, `OpMidiEvent`). A typed way for a Rust host to send events and transport is an addition for after 1.0. |
| `block` (`OpProcess`, `OpEvent`, `OpTransport`, `OpMidiEvent`, the `EVENT_*` and `TRANSPORT_*` constants) | hide | The Rust spelling of `op_shell.h` (F4). |
| `UnsupportedRate` | keep | Added with this change: the error of `HostSession::new`. |

### `openplugin-testing`

Added after the review and publishable, so part of the surface.

| Item | Decision | Applied |
|---|---|---|
| `conformance`, `Options`, `Report`, `Check`, `CheckResult`, `Failure`, `Status`, `HOST_BLOCK_SIZES`, `HOST_SAMPLE_RATES`, `install_allocator!` | keep, mark | `Check` and `Status` are `#[non_exhaustive]`: a new check is a minor release. A plugin that passes today may fail a check added later; that is the point of the kit and is not a breaking change. |
| `alloc` (`CountingAllocator`, `Counts`, `measure`, `is_installed`), `rng` (`Rng`, `derive_seed`) | keep | |
| `bench` (`bench`, `cli`, `compare`, `BenchOptions`, `BenchReport`, `BenchCase`, `Comparison`, `Row`, `Verdict`) | keep, mark | `Verdict` is `#[non_exhaustive]`. The JSON a report writes is read back by `compare`: fields are only added. |

### `openplugin-port`

Not published (`publish = false`): a command-line tool and its library, outside the frozen surface. The table it writes uses the frozen API (`PluginInfo::new`, `with_flag`), and so does the crate `openplugin-port new` writes (`scaffold`): `process` takes channel slices, and the identity pins and state helpers are imported by name beside the prelude.

## What was not reviewed

- Items inside `#[cfg(test)]`, `tests/`, `examples/` and `native/` (the C++ headers `op_shell.h` and `op_editor.h` are the other half of the ABI and were read only to confirm the constants and the ABI number).
- Behaviour. This is a review of names, shapes and exposure, not of correctness.
- Non-macOS builds: rustdoc was run on macOS, so items with a `cfg` for another operating system do not appear. `window::on_main_thread` has a non-macOS branch that was read in the source.
- Trait implementations (`Clone`, `Debug`, `Serialize`, ...) were not graded item by item. Types that lack `Debug` or `Default` are a documentation-pass matter, with `missing_debug_implementations` as the lint.
