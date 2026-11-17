# Versioning and breaking changes

This is the policy for what a version number of OpenPluginSDK promises. It covers the seven publishable crates (`openplugin`, `openplugin-core`, `openplugin-dsp`, `openplugin-dri`, `openplugin-targets`, `openplugin-host`, `openplugin-testing`), which share one version. Releases are recorded in [../CHANGELOG.md](../CHANGELOG.md). What the 1.0 freeze covers is in [The frozen surface](#the-frozen-surface) below; the review that led to it, item by item, is [API_REVIEW.md](API_REVIEW.md).

## Semantic versioning

The SDK follows [Semantic Versioning 2.0.0](https://semver.org/). All crates are released together under one number, so a plugin depends on one version of all of them.

### Before 1.0

Versions are `0.MINOR.PATCH`, with a pre-release suffix while the SDK is in beta (`0.3.1-beta`).

- Any release may contain a breaking change. There is no compatibility promise between two `0.x` releases.
- A breaking change goes into a **minor** release (`0.3` to `0.4`), never into a patch release (`0.3.1` to `0.3.2`). A patch release fixes bugs and adds nothing a plugin must adapt to.
- Every breaking change is listed in the changelog under Changed or Removed and marked **Breaking**, with what a plugin must do. This is how 0.3.1 recorded `write_info`/`write_param` becoming `unsafe`, the sample-rate range and the knob sweep.
- The `-beta` suffix stays until 1.0. Cargo treats `1.0.0-beta` as older than `1.0.0`, so the release that drops the suffix is the first stable one.

### From 1.0

Versions are `MAJOR.MINOR.PATCH`.

- **Major:** a breaking change in any of the areas listed below.
- **Minor:** new features that keep every existing plugin building and behaving the same, and any deprecation. Also the only place the minimum Rust version may rise (see below).
- **Patch:** bug fixes, documentation, and performance work that changes no promised behaviour.
- A major release is rare and planned. Before one, the previous minor release carries deprecations (see Deprecation) so that a plugin can move over a release at a time.
- The latest release in each major line gets security fixes. Older lines are not maintained after the next major release unless the changelog says so.
- A published version is never rewritten. A bad release is yanked on crates.io and fixed by a new patch release.

## What counts as breaking

A change is breaking when a plugin that built, ran and loaded in a host under the previous version would not build, would behave differently, or would no longer be recognised by a host or find its saved data. The areas below are the SDK's compatibility surface. Anything not listed (internal modules, tests, scripts, examples, the build scripts' output layout) is not covered, unless the documentation promises it.

### 1. The public Rust API

Everything in [the frozen surface](#the-frozen-surface). Breaking:

- Removing or renaming a public item, a module, a re-export or a Cargo feature.
- Changing a function or method signature, a generic bound, a return type, or making a safe function `unsafe` (as `write_info` and `write_param` were in 0.3.1).
- Adding a **required** method to `Plugin` or `Editor`, or any other trait a plugin implements. Adding a method with a default body is not breaking, provided the default behaves as the SDK did before.
- Adding a variant to a public enum, or a public field to a struct that plugins can build with a struct literal (`ParamInfo` gained `steps` and `flags` that way in 0.1.1), unless the type is `#[non_exhaustive]`. The types the SDK expects to grow are marked so ([The frozen surface](#the-frozen-surface)), which makes adding to them a minor change.
- Changing a constant's value (for example `MIN_SAMPLE_RATE`), or a documented numeric limit.
- Changing a documented realtime guarantee: `process` must not allocate, lock or panic. Making it do so is breaking even though no signature changes.
- Changing the default behaviour of a provided trait method that plugins rely on without overriding it (for example the default of `Plugin::accepts_sample_rate`).
- Changing the public API of the one dependency the SDK re-exports, `goodlookindri` (`openplugin_dri::goodlookindri`, and `KnobTier`, which is its `KnobMode`): a major version bump of GoodLookin DRI is a breaking change of the SDK unless the re-export is removed or hidden first.
- Raising the edition requirement for plugin code. The SDK itself is edition 2024; plugins may stay on 2021, and that stays true while the export macros stay compatible with it.

Not breaking: adding a new item, module, function, enum variant on a `#[non_exhaustive]` enum, or trait method with a default; fixing a documented-but-wrong behaviour (a bug); making an item more permissive; changes inside private items.

### 2. The C ABI

The shell ABI is a separate contract between a plugin binary built with `export_shell!` and the native VST3, Audio Unit and AAX adapters (`native/include/op_shell.h`, `native/include/op_editor.h`). The adapters and the plugin may be built at different times, so the ABI is versioned on its own: `OP_SHELL_ABI` (currently 8). Adapters refuse a plugin whose `abi` differs.

- Breaking, and requires `OP_SHELL_ABI` to increase: changing the signature, meaning or return values of any `op_shell_*` or `op_editor_*` function; changing the layout or the order of `OpShellInfo`, `OpParamDesc` or `OpEditorHost`; changing a constant's value (`PARAM_*`, `STATE_*`, flag bits); changing the threading or allocation rules in [SHELL.md](SHELL.md).
- Not breaking, and `OP_SHELL_ABI` stays: adding a new function (as `op_shell_save_state` and `op_shell_load_state` were added while the ABI stayed 1), provided a shell that does not call it behaves as before and the header declares it without a guard. Adding a field to a struct breaks the layout, so structs only grow at the end with a `size` the caller sets, or through a new function.
- An ABI increase is a major release after 1.0. Before 1.0 it goes in a minor release. The old version is not kept working in the same binary; adapters and plugin are rebuilt together by `scripts/build_plugin.sh`.
- The CLAP and LV2 entries follow the formats' own specifications (CLAP 1.x, LV2 1.x). Moving to a newer CLAP or LV2 extension version is a minor change when the old one is still honoured.

### 3. Saved-state formats

Anything a host stores for a plugin and hands back later, possibly years later, must keep loading. These formats are part of the compatibility surface:

| Format | Where | Current version |
|---|---|---|
| OPST parameter snapshot | `encode_params` / `decode_params`, CLAP and LV2 state, shell parameter state | magic `OPST`; version 1 (`SNAPSHOT_VERSION`, positional) for a table without identity pins, version 2 (`SNAPSHOT_VERSION_KEYED`, each value under its parameter's string id) for a table with pins. Both are read; a version 1 plane with fewer values than an unpinned table has parameters is an older build's and fills the first parameters |
| VST3 component state head | `native/vst3` `getState` / `setState` / `setComponentState` | version 1 (positional) without pinned parameter ids, version 2 (by `ParamID`) with them; a pinned shell reads both, and an unpinned one reads a version 1 head with fewer values as an older build's |
| OPCH session chunk | `encode_session` / `decode_session`, `op_shell_save_state`, `HostSession::save_bytes`, CLAP state of a plugin with a chunk | magic `OPCH`, version 1 (`SESSION_VERSION`) |
| AAX state chunk | `native/aax` | version 1 (version 2 is planned in [ROADMAP.md](ROADMAP.md)) |
| Preset text | `key = value` files read by `PresetBank` and `preset::parse` | the format of the `openplugin_core::preset` module. It has no version line and is frozen as it is: a new key or value form may be accepted, and every file that parses today keeps parsing to the same values |

Rules:

- A new version of a format may be written only if every older version still loads. Dropping an old version is a breaking change, and a major release.
- A state written by a newer version must be refused cleanly (all or nothing, parameters untouched) by an older reader, never half-loaded. This is already the rule for `decode_params`, `decode_session` and `op_shell_load_state`.
- The version number is bumped whenever the byte layout changes, never reused.
- A plugin's own chunk (`Plugin::save_state` / `load_state`) is the plugin's format, not the SDK's. The SDK promises only to carry it unchanged.
- The OPST snapshot of a plugin without identity pins is positional: it stores one value per parameter in descriptor order. A later build may add parameters at the end of the table: a snapshot with fewer values loads onto the first parameters and the rest take their defaults; one with more values, or none, is refused ([SHELL.md](SHELL.md#older-states-of-a-plugin-without-pins)). So the parameter order of a released, unpinned plugin is part of its state format (see parameter ids below), and only the end of its table may grow. A plugin with pins writes version 2, which names each value by `ParamInfo::id`: a later build takes each value by id, gives a parameter the state does not name its default, and drops a value it has no parameter for. Which of the two a table writes (`identity::state_is_keyed`: any pin, and unique string ids) is part of the contract, as is the rule for a version 1 snapshot read by a pinned table: value `i` goes to the parameter whose CLAP id is `i`, because in the unpinned build that wrote it the position was the id.

### 4. Plug-in identity

Hosts and sessions find a plugin by identifiers. Changing one makes the host treat the plugin as a different one, and sessions lose it. For a plugin built with the SDK these identifiers come from the plugin's descriptor, and the SDK must derive them identically in every release:

- **CLAP:** `PluginInfo::id` is the CLAP plugin id. The SDK passes it through unchanged. `Plugin::clap_id` replaces it.
- **LV2:** the URI is `Plugin::lv2_uri` or, when that is `None`, `PluginInfo::id` (`openplugin_targets::lv2::plugin_uri`). The `.ttl` writer, the port layout (ports 0 and 1 in, 2 and 3 out, one control port per parameter after them) and symbol names derived from parameter ids are part of the URI's contract: a saved LV2 session maps ports by index and symbol.
- **VST3:** the processor and controller class ids are 16 bytes from a 128-bit mix of the plugin id (`mix` in `openplugin-host/src/shell.rs`). The mix, its constants and the salt per id must never change. `Plugin::vst3_ids` replaces them with fixed ids.
- **AAX:** the manufacturer, product, mono and stereo codes are four-byte values from the same mix, with bit `0x20` set in each byte. `Plugin::aax_ids` replaces them with fixed codes. The derivation must never change. A released plugin should set `aax_ids` and `lv2_uri` explicitly, so that renaming `PluginInfo::id` cannot change them.
- **Audio Unit:** the type, subtype and manufacturer codes are build options of `native/CMakeLists.txt` (`OP_AU_SUBTYPE`, `OP_AU_MANUFACTURER`), not derived from the id. Their defaults and names are part of the build contract; a released plugin sets its own and keeps them. `Plugin::au_ids` puts the subtype and manufacturer in the source: the build takes them when it is given none, and stops when it is given others.
- **The id itself:** the SDK cannot stop a plugin author from renaming `PluginInfo::id`, but it must say in the documentation that doing so is a different plugin in every format.

A change to any derivation above is breaking, and in practice is not allowed after 1.0 except as an opt-in alternative. The identity pins (`openplugin_core::identity`, [PORTING.md](PORTING.md#identity-pinning)) are such alternatives: each is `None` by default, and a plugin without one has the derived identifier, bit for bit (the examples' are held as literals in their `tests/identity.rs`). How a pin is turned into what the host sees (the bytes of `Vst3Ids`, the four characters of a code, a parameter pin used as the id unchanged) is itself part of the contract, as are the rules for which pins are usable (`identity::check_pins`): relaxing one would change the ids of a plugin whose unusable pins are ignored today.

### 5. Parameter ids and order

- `ParamInfo::id` is the stable name of a parameter: presets (`preset::parse` matches it, then the display name), `Plugin::set_param` and the LV2 port symbols use it. The SDK never changes how an id is read.
- The **host-facing numeric id** of a parameter without a pin is its index in `PluginInfo::params`: CLAP uses it as `param_id`, VST3 as `ParamID`, the Audio Unit as `AudioUnitParameterID`, LV2 puts control ports after the four audio ports in that order, and OPST version 1 stores values in that order. So inserting, removing or reordering parameters of a released plugin without pins breaks automation and saved sessions. This is the plugin author's responsibility.
- **Pinned ids** (`ParamInfo::with_host_id`, `with_clap_id`, `with_vst3_id`, `with_au_id`, `with_aax_id`) are the stable-id mechanism: a pinned parameter keeps its id wherever it is in the table, and a pinned table saves its state by parameter. It is an addition, not a change of the index rule: a parameter without a pin still has its index as its id, and the shell ABI still addresses every parameter by index. A released plugin moves to pins by pinning each parameter at its released index; after that the table may be reordered and extended, and removing a parameter only drops its saved value.
- The maths that turns a stored value into a normalised one (`ParamInfo::to_normalized` / `from_normalized`, the tapers, the clamping) is part of the contract: hosts store normalised values for automation, so changing it changes what a saved value means. Such a change is breaking. The text from `ParamInfo::format` and `parse` is display only and may be improved in a minor release.

## The frozen surface

This is what the public Rust API of section 1 is, from 1.0. It was fixed by applying [API_REVIEW.md](API_REVIEW.md), which lists every item.

**In the surface:**

- Every public item of the seven crates that rustdoc shows: an item that is not `#[doc(hidden)]`, in a module that is not `#[doc(hidden)]`, with the crate's default features.
- Both paths to an item that has two: its module (`openplugin_core::layout::IoConfig`) and the crate root (`openplugin_core::IoConfig`). The root path is the documented one. Items do not move.
- The facade: `openplugin::core`, `dsp`, `dri`, `targets`, `host`, with the feature `testing` also `testing`, and `openplugin::prelude`, which holds twenty names (`Category`, `Channels`, `ChannelsMut`, `Editor`, `EditorFactory`, `EditorHost`, `EditorState`, `Frame`, `IoConfig`, `MeterFrame`, `NotePorts`, `ParamFlags`, `ParamInfo`, `Plugin`, `PluginInfo`, `SpeakerLayout`, `Tail`, `Taper`, `Transport`, `Unit`) and the macros `export_clap!`, `export_lv2!` and `export_shell!`. A name may be added to the prelude in a minor release; none is removed.
- The three export macros and their two argument forms (`export_clap!(P)` and `export_clap!(P, editor = make_editor)`, the same for the other two).
- The traits a plugin implements: `Plugin` (six required methods: `info`, `prepare`, `reset`, `set_param`, `param`, `process`) and `Editor` (three: `size`, `param_changed`, `render`). Every other method has a default, and every method added from now on will have one that behaves as the SDK did before it.
- `Plugin::process(&mut self, io: &mut [&mut [f32]], frames: usize)`: one slice per channel, each at least `frames` long, and no allocation, lock or panic inside it.

**Types that may grow in a minor release.** These are `#[non_exhaustive]`: a `match` on one needs a wildcard arm, and the structs are not built with a struct literal.

- Structs: `PluginInfo` (built with `PluginInfo::new` and `with_*`), `ParamInfo` (`ParamInfo::new` and `with_*`), `Transport` and `TrackInfo` (from `default()`), `MeterFrame` (from `MeterFrame::SILENT`), `Frame` (`Frame::new`), `openplugin_dsp::MeterReadings`.
- Enums in `openplugin-core`: `Category`, `Unit`, `Taper`, `GroupError`, `Gesture`, `Key`, `AccessRole`, `AccessAction`, `ParamMenuAction`, `SpeakerLayout`, `PortKind`, `Tail`, `Expression`, `midi::Message`, `MidiError`, `MpeEvent`, `PinError`, `PinFormat`, `PresetError`, and in `foreign_state` `ErrorKind`, `ForeignId`, `ValueKind`, `plist::Value` and `xml::Node`.
- Every public enum of `openplugin-dsp`.
- In `openplugin-dri`: `DrawCmd`, `MeterStyle`, `ControlKind`, `DoubleClick`, `DesignEditorError`, `IssueKind`, `Look`. In `openplugin-testing`: `Check`, `Status`, `bench::Verdict`.

Every other public struct and enum is complete as it is, and adding a field or a variant to one is a major change.

**Outside the surface.** These are `pub` because the export macros, the native shells or the SDK's own tests need to reach them, and may change in any release:

- In `openplugin-targets`: the modules `clap`, `clap_abi` and `shell`, `ClapAdapter`, `Lv2Harness`, and in `lv2` the descriptor structs and the macro support. The contracts there are the CLAP and LV2 specifications and the C headers of section 2, not these Rust items.
- In `openplugin-host`: the modules `shell` and `block`, and `HostSession::process_block` and `pull_midi_out`, which take the C ABI's structs.
- In `openplugin-core`: `GestureQueue` and `ValueMirror`, and the `automation` module, which is behind the Cargo feature `automation` (off by default).
- In `openplugin-dri`: the test hooks of the editor views, `DrawList::append_with_basic` and `SceneCanvas::last_scene`.
- In `openplugin-dsp`: `adaa::log_cosh`, `loudness::lufs_of` and `energy_of`, and `KWeighting::coefficients`.
- `openplugin-port` and the examples, which are not published.

**Deprecated at the freeze.** These keep working, with their behaviour unchanged, through every 1.x release, and are removed in 2.0: `Plugin::tail_samples` (override `tail`), and in `openplugin-dsp` `Compressor` (`Dynamics`), `VuMeter` (`VuIec`), `Oscillator` and `Wave` (`BandlimitedOsc`, `WavetableOsc`), `SampleVoice` (`SincPlayer`), `Polyphony` (`VoiceAllocator`), `Oversampler2x` (`MinPhaseHalfband`) and `window::hann` (`WindowKind::Hann`).

**Sample rates.** A rate is an `f32` everywhere a plugin or a DSP block sees one (`Plugin::prepare`, `accepts_sample_rate`, `MIN_SAMPLE_RATE`, `MAX_SAMPLE_RATE`). It is an `f64` only where a host hands one over (`sample_rate_supported`, `HostSession::new` and `prepare`, the C ABI), because the range is checked before the value is narrowed.

**The GoodLookin DRI dependency.** `openplugin_dri::goodlookindri` is re-exported so an editor can build scenes; its types appear in `DrawList::scene` and `KnobTier`. The style codes `draw_knob` and `draw_faceplate` take are GoodLookin DRI's; a code that draws today keeps drawing.

## Deprecation

Before 1.0 the SDK may remove without a deprecation step, but prefers one when the cost is small. From 1.0 a removal needs this sequence:

1. **Announce.** In a minor release the item gets `#[deprecated(since = "1.x.0", note = "use ... instead")]`, the replacement is available in the same release, and the changelog lists it under Deprecated.
2. **Warn for at least one full minor release and not less than three months.** The compiler warning is the notice, so it names the replacement. For a C ABI function or a saved-state format, the deprecation is documented in `SHELL.md` or `FORMATS.md` since there is no attribute.
3. **Remove in the next major release,** listed in the changelog under Removed and marked **Breaking**, with a migration note.

Other rules:

- A deprecated item keeps its behaviour. A deprecation never changes what the old call does.
- A security or soundness fix may skip the sequence. It is then announced under Security in the changelog.
- An item that is `#[doc(hidden)]`, or behind the `automation` Cargo feature of `openplugin-core`, is outside the policy. This is where the items live that are internal but must be `pub` for the export macros, the native shells and the SDK's own tests.
- A plugin author can find out about upcoming breakage by building with `-D deprecated` in CI.

## Minimum supported Rust version

The minimum Rust version, why it is what it is, and when it may rise, are written down in the section "Minimum Rust version" of [ROADMAP.md](ROADMAP.md#minimum-rust-version). This policy adds only the versioning consequences:

- The minimum is `rust-version` in the workspace `Cargo.toml` (1.85 in 0.3.1), and CI checks the workspace on exactly that version.
- Raising it is allowed only in a **minor** release, never in a patch release, and is not treated as a breaking change that needs a major release. The changelog states the new minimum and the reason under Changed, marked **Breaking** before 1.0.
- After 1.0, a plugin that pins an older Rust stays able to use the previous minor release of the SDK, which is why the minimum may rise in minor releases only and a patch release for a previous minor line will not raise it.
- Dropping support for edition 2021 plugins (the export macros requiring a newer edition) is a major change.

## How a release is made

1. Every pull request that changes anything in the surface above updates `CHANGELOG.md` under Unreleased, in the right category.
2. A breaking change is reviewed against this document; the changelog marks it **Breaking**, and the commit message says why.
3. The release commit bumps `version` in the workspace `Cargo.toml` and every `openplugin-*` entry in `[workspace.dependencies]`, moves Unreleased to a dated section, and updates the version badge in `README.md`.
4. The release is tagged `vMAJOR.MINOR.PATCH[-suffix]`. Only `v0.1.0-beta.1` is tagged so far; from the next release every release gets a tag.
5. Publishing to crates.io, the 1.0 freeze and any yank are the owner's decisions.

The release steps for publishing, in crate order, are in [PUBLISHING.md](PUBLISHING.md).
