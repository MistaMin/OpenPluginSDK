# Porting toolkit: moving a plugin from another SDK

`openplugin-port` is a set of tools for moving a plugin that already ships in CLAP, VST3, Audio Unit or AAX onto OpenPluginSDK without losing what its users depend on: the same parameters, in the same ranges, with the same identity, so sessions and presets keep working. Each tool answers one question and writes something a person can read and check. None of them edits your plugin or installs anything.

Today the toolkit has three tools: the scanner (porting task 1), which answers "what is this plugin?", the null test (porting task 4), which answers "does the port sound the same?", and the state import (porting task 3), which answers "will the port open the old plugin's sessions and presets?". The tasks that follow build on the scanner's manifest.

The [porting guides](#porting-guides) (porting task 5) say how to write the port itself: one guide for each SDK a plugin is likely to come from, mapping its concepts onto this SDK's, with a small effect ported in full.

Shipping before the DSP is rewritten? [CPP_BRIDGE.md](CPP_BRIDGE.md) shows how to call your existing C++ DSP from a Rust plugin through `cxx` (porting task 7), with the real-time rules and how to test them.

| Task | Tool | State |
|---|---|---|
| 1 | `scan`: read an existing plugin into a manifest and a Rust snippet | this document |
| 4 | `null`: render the original and the port through the same input and automation and compare | [The null test](#the-null-test) |
| 3 | `capture` and `openplugin_core::foreign_state`: save the old plugin's state for each preset, and read the containers it is made of | [Foreign state import](#foreign-state-import) |
| 5 | Guides: iPlug2, DPF, nih-plug, JUCE and the raw VST3, CLAP and Audio Unit SDKs mapped onto this SDK, each with a worked port | [Porting guides](#porting-guides) |
| 2 | Pin identity the SDK derives today (explicit VST3 parameter ids, class ids) | later; the scanner marks each spot with a `TODO(porting task 2)` |

## Porting guides

One guide for each SDK a plugin is likely to come from. Each maps that SDK's processor, parameters, state, editor, buses, MIDI and threading onto this SDK's in tables you can read beside your own source, then ports a small effect in full, and ends with what has no counterpart yet.

| Coming from | Guide | Worked port |
|---|---|---|
| iPlug2 | [porting/iplug2.md](porting/iplug2.md) | A smoothed gain with a peak meter, mono and stereo, and a small editor |
| DPF (DISTRHO Plugin Framework) | [porting/dpf.md](porting/dpf.md) | A low-pass filter whose programs become factory presets |
| nih-plug | [porting/nih-plug.md](porting/nih-plug.md) | A ducker driven by a sidechain bus |
| JUCE | [porting/juce.md](porting/juce.md) | An echo with a tail that opens the old plugin's XML state |
| VST3 SDK | [porting/vst3.md](porting/vst3.md) | A tremolo locked to the host's tempo and position |
| CLAP SDK | [porting/clap.md](porting/clap.md) | A gate opened by notes |
| Audio Unit SDK (version 2 and version 3) | [porting/audio-unit.md](porting/audio-unit.md) | One band of parametric EQ with a reported tail, mono and stereo |

The guides are written from each project's public documentation and public API names. They contain no code from those projects: every code block is Rust written for this SDK.

**The code in the guides cannot rot.** Each guide is the documentation of a module of `examples/porting_guides`, so every code block is a doc test: it is compiled against the current SDK, runs its own checks (the gain, the filter's response, the echo's delay, the state import) and then runs the port through the [conformance kit](TESTING_PLUGINS.md) with the allocation check on. Unit tests in that crate fail if a guide gains a code block that is not a Rust doc test, if a file in `docs/porting` is not compiled, or if this table loses a link.

```bash
cargo test -p example-porting-guides
```

### Order of work

1. **Scan the old plugin.** `openplugin-port scan` ([The scanner](#the-scanner)) writes its identity, buses, latency, tail and every parameter, and a Rust snippet with the `ParamInfo` table. Keep the parameters in the old order: the SDK numbers them by table position in every format.
2. **Write the `Plugin`.** Read the guide for your SDK beside your source. Start with `info`, `prepare`, `reset`, `set_param`, `param` and `process`, then layouts, notes, latency and tail. Allocate only in `prepare`.
3. **Pass the conformance kit.** `openplugin_testing::conformance` ([TESTING_PLUGINS.md](TESTING_PLUGINS.md)) runs the port through what hosts do: every sample rate and block size, NaN at the input, parameter fuzz, state round-trips, and a check that the audio path does not allocate. Every worked port in the guides ends with it.
4. **Decide what happens to old sessions.** `openplugin-port capture` ([Foreign state import](#foreign-state-import)) saves the old plugin's state for each preset. If the port must open it, write the import in `load_state` and a test that loads every captured blob.
5. **Null against the original.** `openplugin-port null` ([The null test](#the-null-test)) renders both through the same input and automation and reports the first differing sample.
6. **Write the editor** ([EDITOR.md](EDITOR.md)).
7. **Build every format and run the validators** ([BUILD.md](BUILD.md), [HOST_TESTS.md](HOST_TESTS.md)).

Still on C++ DSP? [CPP_BRIDGE.md](CPP_BRIDGE.md) lets the port ship first and the DSP move later.

### What a port cannot carry over yet

Each guide lists what its SDK has that this one lacks. The ones that matter for every port:

- **The old plugin's identity outside CLAP, LV2 and the Audio Unit.** The CLAP id is `PluginInfo::id`, the LV2 URI is `Plugin::lv2_uri`, and the Audio Unit subtype and manufacturer codes are build options, so those carry over. The VST3 class ids are derived from `PluginInfo::id`, and parameters are numbered by table position in CLAP, VST3 and the Audio Unit, so a plugin whose parameter ids were not `0..n` in order cannot keep them. This is porting task 2.
- **The old plugin's saved state through the shells.** The CLAP entry hands `load_state` the stream as the host stored it, for a plugin that saves a chunk of its own. The VST3, Audio Unit and AAX shells accept only state they wrote themselves; the Audio Unit shell does restore the parameters of a dictionary the old unit's base class wrote. Also porting task 2.
- **Parameter values in a plugin's own chunk.** A plugin whose `save_state` returns a chunk has that chunk stored alone by the CLAP entry, and stored beside the parameter values by the shells. Until the entries agree, a chunk must hold the parameter values too.
- **Factory presets in the host.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-ffi`; no format entry publishes a program list yet.
- **Parameter travel other than linear and logarithmic, and custom value text.** `Taper` has two shapes, and a value prints and parses by its `Unit` and choice names.
- **More than one output bus or sidechain, 64-bit audio buffers, and SysEx.**

## The scanner

```bash
cargo build --release -p openplugin-port
target/release/openplugin-port scan "/Library/Audio/Plug-Ins/VST3/Some Reverb.vst3" --out port-out
```

```
openplugin-port scan <plugin>... [--out DIR] [--timeout SECONDS] [--format FORMAT]
```

- `<plugin>` is a `.clap`, `.vst3`, `.component` or `.aaxplugin` path (one or several). The format comes from the extension; `--format clap|vst3|au|aax` overrides it, which also lets a bare library be scanned as CLAP.
- `--out DIR` receives, for each plugin found, `<name>.<format>.manifest.json` and `<name>.<format>.params.rs` (default: the current folder). `<name>` is the bundle's file stem, or the plugin's name when a bundle holds several plugins.
- `--timeout` is how long each plugin gets (default 30 seconds).
- One line per plugin is printed (`ok` or `FAIL` with the reason). The exit code is 0 when every plugin scanned.

The plugin is loaded from where it is. Nothing is copied, registered with the system, or installed.

### How each format is read

| Format | What the scanner does |
|---|---|
| CLAP | `dlopen` of the bundle's executable, `clap_entry`, the plugin factory, then for each plugin: `clap.params`, `clap.audio-ports`, `clap.latency`, `clap.tail`. Parameter text comes from `value_to_text`. The CLAP ABI is the one `openplugin-targets` already defines. |
| VST3 | The bundle is loaded through CoreFoundation (`bundleEntry`, `GetPluginFactory`). The COM interfaces `IPluginFactory`, `IPluginFactory2`, `IPluginFactory3`, `IComponent`, `IAudioProcessor` and `IEditController` are bound by hand in `src/vst3.rs` from the MIT-licensed public interface headers of the VST3 SDK (no other VST3 binding crate is used). The scanner plays a minimal host (`IHostApplication`, `IComponentHandler`), creates the component and the controller, activates the buses and reads parameters, buses, latency and tail. `Contents/Resources/moduleinfo.json` is merged in when present. |
| Audio Unit | The bundle's `Info.plist` `AudioComponents` entry gives type, subtype, manufacturer and the `factoryFunction`. The bundle's code is loaded into the scanner's child process, registered there with `AudioComponentRegister` (the registration lives and dies with that process), and one instance is asked for its parameter list, parameter info, value strings, buses, channel layouts, latency and tail. A unit with no `factoryFunction` (an old Component Manager unit) can only be read if the system already knows it, and the manifest says so. The AudioToolbox calls are declared by hand in `src/sys.rs`. |
| AAX | Not hosted: the AAX SDK is not redistributable and an AAX plug-in needs Pro Tools. The scanner reads `Info.plist` and any page-table XML in `Contents/Resources` for `manID`, `prodID` and `plugID`, and lists the parameter ids the page table names. A bundle without a page table gets a warning: its ids are inside the compiled binary. |

### Isolation

A plugin is only ever loaded in a child process. The scanner re-executes itself with a hidden subcommand (`openplugin-port __probe <format> <path>`); the child loads the plugin, prints one result line, and leaves with `_exit` so the plugin's teardown (the likeliest thing to crash) never runs. The parent reads the child's output while it waits, kills it at the timeout, and turns a signal into a message (`the plugin crashed the probe process (SIGSEGV...)`). Noise the plugin prints is ignored: only the marked result line counts. The tests inject a crash and a hang through `OPENPLUGIN_PORT_FAULT=crash|hang` (read by the child only, for tests) and check that the scan reports the failure and the next scan works.

The scanner does what a host does when it loads a plugin: the plugin's own code runs, and a plugin that writes files when it loads (a licence check, a cache) still does. Scan only plugins you are willing to load.

## The manifest

One JSON file per plugin. `schema_version` is 1; a reader should refuse a higher number than it knows. A change that removes a field or alters its meaning bumps the version; adding a field does not.

```jsonc
{
  "schema_version": 1,
  "tool": "openplugin-port 1.0.0-beta",
  "source": {
    "format": "vst3",              // clap | vst3 | au | aax
    "path": "/abs/path/Some Reverb.vst3",
    "index": 0, "count": 1,        // this plugin's place among those the bundle holds
    "bundle_id": "com.acme.verb"   // CFBundleIdentifier, or null
  },
  "plugin": {
    "name": "Some Reverb", "vendor": "Acme", "version": "1.2.0",
    "url": "", "email": "", "description": "",
    "categories": ["Fx", "Reverb"],  // CLAP features, VST3 sub-categories, or the AU type
    "is_instrument": false,
    "ids": {                          // each is null when the format does not apply
      "clap": { "id": "com.acme.verb" },
      "vst3": {
        "class_id": "34D4...6BBE",              // 32 hex digits: the 16 TUID bytes in order
        "controller_class_id": "4F46...0BDF",   // null for a single-component plugin
        "category": "Audio Module Class", "sub_categories": "Fx|Reverb",
        "class_flags": 0, "sdk_version": "VST 3.7.8",
        "single_component": false,
        "module_info": true,                    // the bundle has moduleinfo.json
        "module_info_class": { /* the matching Classes entry, as found, or null */ }
      },
      "au": {
        "component_type": "aufx", "subtype": "ExGn", "manufacturer": "OpGd",
        "factory_function": "OpenPluginAUFactory", "version": 1
      },
      "aax": {
        "manufacturer": "OpGd", "product": "EdGn",
        "plugin_ids": ["EdGM", "EdGS"],         // in page-table order
        "found_in": "page-table"
      }
    },
    "buses": [
      { "direction": "input", "kind": "main",   // input | output; main | aux
        "name": "Input", "channels": 2,
        "arrangement": "0x3",                   // VST3 speaker arrangement, or the CLAP port type; may be null
        "layout": "stereo" }                    // mono | stereo | null
    ],
    "channel_layouts": [ { "inputs": 2, "outputs": 2 } ],  // Audio Unit SupportedNumChannels; -1 any
    "latency_samples": 0,        // null when not reported
    "tail_samples": 0,           // 4294967295 is endless; null when not reported
    "tail_seconds": null,        // Audio Unit only
    "params": [ /* see below */ ],
    "warnings": []               // anything the scan could not read or had to guess
  }
}
```

A parameter:

```jsonc
{
  "id": 0,                       // the format's own id: CLAP param_id, VST3 ParamID, AU parameter id
  "name": "Gain", "short_name": null,
  "group": null,                 // CLAP module path, VST3 unit id, AU clump id
  "units": "dB",                 // unit text from VST3 or the AU unit; "" when none (CLAP has none)
  "min": -24.0, "max": 24.0, "default": 0.0,   // plain values as the format reports them
  "default_normalized": 0.5,
  "step_count": 0,               // 0 continuous, 1 toggle, N for N+1 positions (the VST3 meaning)
  "flags": ["automatable"],      // see below
  "flags_raw": 32,               // the format's own flag word, unchanged
  "samples": [                   // value strings at normalised 0, 0.25, 0.5, 0.75, 1
    { "normalized": 0.0, "plain": -24.0, "text": "-24.0 dB" }
  ],
  "step_texts": []               // discrete parameters with up to 63 steps: the text at every position
}
```

Flag names: `automatable`, `stepped`, `hidden`, `read_only`, `bypass`, `list`, `wrap_around`, `program_change`, `requires_process`, `periodic`, `non_realtime`, `meter`, `expert`, `preset_exclude`, and for an Audio Unit's display curve `log_display`, `exp_display`, `sqrt_display`, `squared_display`, `cubed_display`, `cube_root_display`.

How "normalised" is read depends on the format, and the manifest keeps both numbers so nothing is lost:

- VST3: `plain` is `normalizedParamToPlain`. A plugin that does not implement it reports `plain` equal to `normalized`; `min` and `max` are then 0 and 1.
- CLAP: parameters are plain values with a range, so the sample points are linear in the plain value.
- Audio Unit: likewise linear in the plain value; a non-linear display shows as a flag (`log_display`).

Audio Unit latency and tail are seconds in the API; the manifest converts them to samples at the sample rate of the unit's stream format and keeps the tail in seconds.

## The Rust snippet

`<name>.<format>.params.rs` has three parts. It is meant to be read and edited, not compiled as it stands: it starts with `use openplugin::prelude::*;` and declares `PARAMS` and `INFO` the way `examples/gain` does.

**The parameter table.** One `ParamInfo::new(...)` per parameter, with a comment giving the source id and, where something was guessed, what and from what. The SDK addresses a parameter by string id, so the id is made from the name (`Gain` becomes `"gain"`; a repeated name gets `_2`). Then:

- **Unit.** From the format's unit text (`dB`, `Hz`, `ms`, `s`, `%`, `:1`, `st`, `ct`), else from the word after the number in the value strings.
- **Toggle and choice.** `step_count` 1 with on/off-like text is `Unit::Toggle`. A discrete parameter whose texts are not numbers becomes `Unit::Choice` with `.with_choices(&[...])`, the labels read at every position. A discrete numeric one gets `.with_steps(step_count + 1)`.
- **Range.** The plain range, unless the value strings are numbers on another scale (a plugin whose plain range is 0 to 1 but prints "20 Hz" to "20 kHz"): then the range comes from the strings and the comment says so. A VST3 plugin with no display function prints the normalised position; the scanner recognises that and ignores the text.
- **Taper.** The five sample points are compared with the line and the exponential curve (`min * (max / min)^t`, which is what `Taper::Log` does) through the end points. The curve is chosen when it fits (error at most 3% of the travel) and the line clearly does not (at least 8%). The series fitted is the plain values when they are not linear in the normalised position (VST3), else the numbers in the value strings. An Audio Unit flagged `log_display` is `Taper::Log`. Everything else is `Taper::Linear`, with a comment saying whether that was a fit or a guess because the plugin gave no usable text (CLAP and Audio Unit plain values are linear by construction, so only their value strings can reveal a curve).
- **Flags.** `HIDDEN`, `READ_ONLY`, `BYPASS`, `NOT_REALTIME` map across. A parameter that is not automatable in the source and has none of those gets `NOT_REALTIME`, because the SDK has no other way to switch `AUTOMATABLE` off, and the comment says so.

**`PluginInfo`.** Name, vendor, URL, e-mail, version, a `Category` chosen from the categories, `is_instrument`. `id` is the CLAP id when there is one; otherwise it is made from the vendor and name and the comment says that is a guess, because the SDK derives the VST3 class ids from it.

**Identity and build settings.** The command-line options for `scripts/build_plugin.sh` (`--name`, `--manufacturer`, `--version`, `--bundle-id`), the Audio Unit codes as the CMake cache variables `OP_AU_SUBTYPE` and `OP_AU_MANUFACTURER` (so `-DOP_AU_SUBTYPE=...`) and as `--au-subtype`/`--au-manufacturer`, a ready `fn aax_ids` for `Plugin` when the page table gives two plug-in types, a `supported_layouts` for plain mono and stereo buses, and the reported latency and tail. Then the things the SDK cannot pin yet, each marked `TODO`:

- the VST3 processor and controller class ids (the SDK derives them from `PluginInfo::id`);
- the parameter ids, when the source's are not `0..n` in table order (the SDK numbers parameters by position in CLAP, VST3 and Audio Unit); this is porting task 2;
- an Audio Unit type other than `aufx` (the shell writes `aufx`);
- main buses wider than stereo, or an odd sidechain, which need a hand-written `supported_layouts`.

## What the proof covers

`crates/openplugin-port/tests/scan.rs` scans the SDK's own example (`examples/editor`) and checks the result against the example's source:

- Always: it builds the example's CLAP library and scans it. The manifest turned into SDK terms is the example's `PARAMS` table exactly (id, name, min, max, default, unit, taper, steps, choices, flags), the CLAP id is `INFO.id`, the generated lines `ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),` and the Warm toggle appear verbatim, and the manifest survives a JSON round trip. A plugin that aborts and one that hangs each fail only their own scan.
- With `OPENPLUGIN_PORT_FIXTURES` set to a folder holding `Example Gain.clap`, `.vst3`, `.component` and `.aaxplugin` as `scripts/build_plugin.sh` writes them (`--formats clap,vst3,au,aax --tail --aax-pages`, which needs the VST3, AudioUnitSDK and AAX checkouts): the VST3 and Audio Unit scans reproduce the same table; the VST3 class ids equal those `openplugin_ffi::shell::write_info` computes; the Audio Unit is `aufx`/`ExGn`/`OpGd` with its factory function; the AAX ids equal the example's `Plugin::aax_ids`.

```bash
F=build/fixtures
scripts/build_plugin.sh --manifest-path examples/editor/Cargo.toml --version 0.3.1 \
  --name "Example Gain" --bundle-id com.opengrid.example --manufacturer OpenGrid \
  --au-subtype ExGn --au-manufacturer OpGd --formats clap,vst3,au,aax --tail --aax-pages \
  --vst3-sdk /path/to/vst3sdk --au-sdk /path/to/AudioUnitSDK --aax-sdk /path/to/aax-sdk \
  --out "$F/dist" --work-dir "$F/work"
OPENPLUGIN_PORT_FIXTURES="$F/dist" cargo test -p openplugin-port
```

### Plugins from other makers

The scanner was also run, read-only, over a spread of plugins installed on the development machine (CLAP, VST3 and Audio Unit from several makers, loaded from where they sit, never copied or changed). On 15 November 2026, 112 plugins were scanned from `/Library/Audio/Plug-Ins` and `~/Library/Audio/Plug-Ins` (every Nth entry of each folder): 14 CLAP, 28 VST3, 45 Audio Unit and 25 AAX. All 112 scanned: no failure, crash or timeout.

| Format | Scanned | Result |
|---|---|---|
| CLAP | 14 | 14 with parameters (312 in all) |
| VST3 | 28 | 28 with parameters (13,283 in all; three plugins expose about 2,100 each) |
| Audio Unit | 45 | 45 with parameters (2,549 in all); 29 report a non-zero latency |
| AAX | 25 | identity only; 8 had a page table with ids, 17 keep them inside the binary |

Across the generated tables, 104 parameters were given `Taper::Log` from their value strings or plain values, and 233 became `Unit::Choice`. A first run over the same VST3 plugins found no parameters in half of them: those plugins hand their controller its state through an `IMessage` sent over `IConnectionPoint`, so the scanner's host now makes messages and attribute lists and connects the component to its controller, as a host does.

## The null test

`openplugin-port null` answers "does the port sound like the original?". It renders the original plugin and the port through the same input and the same parameter automation, aligns them by their reported latencies, subtracts, and exits non-zero when the difference is over a tolerance.

```bash
openplugin-port null "Old Verb.vst3" "New Verb.vst3" \
  --input noise:3 --automation "0:Mix=0.3, 1s:Mix=0.8, 1.5s:Size=40" \
  --tolerance 1e-6 --save out/
```

```
openplugin-port null <original> <port> [--format FMT] [--port-format FMT]
    [--input SIGNAL|file.wav] [--automation SPEC] [--rate HZ] [--block N]
    [--tolerance T] [--param-map map.json] [--save DIR] [--json FILE] [--timeout SECONDS]
openplugin-port param-map <original> <port> [--out map.json]
```

- **Plugins.** Each is a CLAP, VST3 or Audio Unit loaded from its path, the way the scanner loads it: in its own child process (`openplugin-port __render`, hidden), with a timeout (`--timeout`, default 120 s), hand-written bindings, nothing installed, the Audio Unit registered in that process only. The two plugins may be different formats (`--format` names the original's and `--port-format` the port's when the extension does not say; the CLI applies `--format` to both). A bundle that holds several plugins renders the first and says so. AAX cannot be hosted offline and is refused.
- **Input.** `--input` is a built-in signal, optionally followed by `:seconds` (default 2): `sweep` (logarithmic, 20 Hz to 20 kHz, 0.5 peak), `noise` (white, 0.25 peak, fixed seed), `impulses` (an impulse every 997 samples), `burst` (silence, then a quarter of two tones, then silence) or `silence`; or a path to a WAV file (8, 16, 24 or 32-bit integer, 32 or 64-bit float, any channel count; read by a small reader in this crate, no dependency). The built-in signals are stereo and deterministic. A plugin's input channel `c` takes source channel `c mod source channels`, so a mono plugin reads the left channel. `--rate` (default 48000, or the WAV file's own rate) must match a WAV file: the tool does not resample. `--block` (default 512) is the host block size.
- **Automation.** `--automation` is a list of `frame:param=value` entries separated by commas, semicolons or newlines; a time is a frame number or ends in `s` or `ms`; `param` is the original plugin's parameter id or name; the value is a plain value in the parameter's own units, set exactly at that frame (a CLAP `param_value` event with that time, a VST3 queue point with that sample offset after the plugin's own `plainParamToNormalized`, an Audio Unit immediate event at that buffer offset). `@file` reads the entries from a file, one per line, `#` for comments. Both plugins get the same frames and values.
- **Parameter map.** The port's parameter for each automated original parameter is found, in order, in the map file (`--param-map`, a JSON object from original id or name to port id or name, such as `{"3": "reverb_mix", "Size": 7}`), then by the same name (ignoring case), then by the same id. The pairs used are printed. `openplugin-port param-map <original> <port>` scans both and writes the map it would use as `{"<original id>": <port id>}`, listing on stderr the original parameters without a match so they can be added by hand. A value is passed on unchanged: if the port's range or units differ, the null test cannot compensate; compare at the defaults or give the port the same plain values.
- **Latency.** Each child reads the plugin's latency after it is activated (CLAP `latency`, VST3 `getLatencySamples`, Audio Unit `Latency` in seconds times the rate), renders the input followed by that many frames of silence, and drops the first `latency` output frames. So a plugin that delays its output by its reported latency lines up with one that does not. The report says `LATENCY DIFFERS` when the two reports are not equal (information, not a failure: a port that reports a different latency but aligns correctly nulls). If a plugin reports a latency it does not have, the aligned renders differ and the null fails, which is the right answer.
- **Output.** For each channel: the maximum absolute difference and where it is, the RMS of the difference in dB (`-inf dB` is exactly zero), the number of samples over the tolerance, the first sample that is not bit-identical and the first over the tolerance, each with both values. `--tolerance` (default `1e-6`) is an absolute sample difference. `--json FILE` writes the same report as JSON. `--save DIR` writes `input.wav`, `original.wav`, `port.wav` and `difference.wav` (aligned, 32-bit float) for listening.
- **Exit code.** 0 within the tolerance; 1 over the tolerance; 3 when a plugin could not be rendered (it would not load, it crashed, it hung past the timeout, the two report different numbers of output channels); 2 for a usage error.

### Reading the report

```
original  Old Verb.vst3 [vst3] "Old Verb": latency 64 samples, 2 in, 2 out
port      New Verb.vst3 [vst3] "New Verb": latency 0 samples, 2 in, 2 out
48000 Hz, blocks of 512, 144000 frames compared, tolerance 1e-6
LATENCY DIFFERS: original 64 samples, port 0; outputs are aligned by their own latency
parameter 3 (Mix) -> 7 (Mix)
channel 0: max |diff| 1.2e-3 at sample 9210, RMS diff -62.40 dB, 8800 samples over tolerance
  first sample over tolerance: 64 original 1.0e-2 port 9.4e-3
FAIL: the port does not null against the original
```

Start with `first sample over tolerance`. A difference at sample 0 of an input that starts at sample 0 is a gain or polarity error; at the latency, a delay that is not what the plugin reports; at the first automation frame, a parameter that does not reach the same value (check the map and the units); only late in the render, state that drifts (a filter or smoothing that differs). A small RMS with a large maximum is a few wrong samples (a block edge: try `--block 64` and `--block 997`); an RMS only a little below the signal is a different process. Listen to `difference.wav`: a faint copy of the input is a gain error, noise is a different algorithm or dither, a click at the automation frames is a smoothing difference. Plugins with random elements (a chorus with an LFO started at a random phase, dithered output) will never null; compare them with a tolerance at the level of the randomness, or switch the randomness off. Processing that is the same maths in a different order differs at about `1e-7`; the default tolerance leaves ten times that.

### What the proof covers

`crates/openplugin-port/tests/null.rs` nulls the SDK's own example (`examples/editor`) against a second copy of the same build, through CLAP always and through VST3 and the Audio Unit when `OPENPLUGIN_PORT_FIXTURES` names the folder of built bundles (the same folder as the scanner tests; `Latency Stub.vst3` and `LatencyTest.component`, the test bundles of `native/CMakeLists.txt` that report 37 samples, are added to it for the latency test):

- The five built-in signals and a seven-event automation (Gain and Warm, including mid-block changes and two parameters at one frame) null with maximum difference exactly 0 in CLAP, VST3 and the Audio Unit, and the render follows the automation (the saved output is the input times -3 dB, then +6 dB, to 1e-4).
- A deliberately changed port fails with the first differing sample reported. The change is a test-only hook of the render child (`OPENPLUGIN_PORT_RENDER_GAIN=1.001`, read only by `__render`, set only on the port's child by the test); no shipped plugin code differs. The report names sample 0 with `port == original * 1.001`, and passes again once `--tolerance` is above the difference. A port whose `Gain` parameter drives the original's `Warm` also fails (exit code 1 from the CLI).
- Latency: the same latency plugin twice nulls with 37 samples on both; the example (latency 0) against the stub (latency 37) reports `LATENCY DIFFERS` and the saved port render is exactly the stub's output shifted by 37 samples, the last 37 being the silence the host appended.
- A plugin that aborts or hangs inside `process` fails the run with `crashed` or `timed out`, and the next run works. The scanner's fault hook `OPENPLUGIN_PORT_FAULT` also reaches the render child (`crash`, `hang`, and `crash-render` and `hang-render`, which fire at the second block).
- WAV input in 16-bit, 24-bit and 32-bit integer and 32-bit float nulls; a `--rate` that disagrees with the file is refused.

### Limits of the null test

- Audio only: no MIDI input, so instruments render silence unless they make sound on their own; no transport (a tempo-synced plugin sees none).
- The main audio buses only; a sidechain is not fed.
- Parameter values are set, not ramped, and only parameters named in the automation move; everything else keeps its default. Plugin state (presets) cannot be loaded into the null test yet; `capture` (see [Foreign state import](#foreign-state-import)) saves it, and loading it into a render is not wired in.
- Both plugins render at the same sample rate and block size, in 32-bit float.
- The latency tests need the test bundles of `native/` built by CMake; without them (and without `OPENPLUGIN_PORT_FIXTURES`) the VST3 and Audio Unit tests print "skipped".

## Foreign state import

A port must open the sessions and presets its users already have. The host hands the port the bytes the old plugin saved, through `Plugin::load_state`. Two tools cover this: `capture` saves those bytes from the old plugin, one set for each of its presets, so you can see what the old format looks like and keep them as test data; `openplugin_core::foreign_state` reads the containers that formats are usually made of.

### Capture

```bash
openplugin-port capture "/Library/Audio/Plug-Ins/Components/Some Synth.component" --out port-out --set cutoff=800 --set Resonance=0.3
```

```
openplugin-port capture <plugin>... [--out DIR] [--format FORMAT] [--set PARAM=VALUE]...
                        [--max-presets N] [--timeout SECONDS]
```

The plugin is loaded in a child process from where it is (nothing is installed or copied; a crash or hang costs one capture, as for the scanner). It is hosted the way the format's hosts do, and each state is saved exactly as a host would store it:

| Format | Blob roles | How the states are made |
|---|---|---|
| CLAP | `clap-state`: the bytes the plugin writes to the `clap.state` stream | Default after activation. Factory presets through the bundle's preset-discovery factory (`clap.preset-discovery-factory/2` or its draft name): the host declares itself as the indexer, the provider's locations are walked (a folder is searched, depth 8, for the declared file extensions), each preset listed for this plugin's id is loaded through the plugin's `clap.preset-load/2` and the state saved. |
| VST3 | `vst3-component`: `IComponent::getState`; `vst3-controller`: `IEditController::getState` (left out when the controller has no state); `vst3-program-data`: `IProgramListData::getProgramData` when offered | Default after both halves are created and connected. Factory presets are the `IUnitInfo` program lists: each program is selected through the list's program-change parameter (set on the controller and sent to the component in a process block, as a host does), then the streams are read. A list with neither a program-change parameter nor program data is noted and skipped. |
| Audio Unit | `au-classinfo`: the `kAudioUnitProperty_ClassInfo` dictionary as a binary property list (`bplist00`), the form hosts write into sessions | Default after `AudioUnitInitialize`. Factory presets from `kAudioUnitProperty_FactoryPresets`, each selected through `kAudioUnitProperty_PresentPreset`. |

`--set PARAM=VALUE` (repeatable) adds one more state, named `set`, from a fresh instance with those parameters moved (the parameter's id or its name, and a plain value). It is how to get a non-default state from a plugin with no presets. `--max-presets` (default 128) bounds the presets taken; the rest are noted, not dropped silently.

After each state is loaded, every parameter's plain value is read back from the plugin (CLAP `get_value`, the VST3 controller's normalised value converted by `normalizedParamToPlain`, `AudioUnitGetParameter`). Those are the numbers a port's loader has to reproduce.

The output is `<name>.<format>.capture.json` and a folder `<name>.<format>.blobs/` holding `NNN-<state>.<role>.bin`, next to each other in `--out`. The manifest:

```json
{
  "schema_version": 1,
  "tool": "openplugin-port 1.0.0-beta",
  "source": { "format": "au", "path": "...", "index": 0, "count": 1, "bundle_id": "com.acme.synth.au" },
  "plugin": { "name": "...", "vendor": "...", "version": "...", "ids": { "clap": null, "vst3": null, "au": { "...": "as in the scanner manifest" }, "aax": null } },
  "states": [
    {
      "name": "default",
      "kind": "default",
      "location": null,
      "blobs": [ { "role": "au-classinfo", "file": "synth.au.blobs/000-default.au-classinfo.bin", "bytes": 222, "fnv1a64": "971e7d5600a61506" } ],
      "params": [ { "id": 0, "name": "Gain", "plain": 0.0, "normalized": null } ],
      "notes": []
    }
  ],
  "notes": []
}
```

- `schema_version` is 1; a reader must refuse a higher number. `plugin.ids` is the scanner manifest's, so parameter ids in `params[].id` are the scanner's `params[].id`.
- `states[].kind` is `default`, `set`, `factory-preset` (from a location the provider marks as factory content, a VST3 program list or an Audio Unit factory preset) or `preset` (CLAP content a provider lists that is not factory content). `location` says where it came from: `plugin#KEY` or a file path (CLAP), `program-list:ID/INDEX` (VST3), `factory-preset:NUMBER` (Audio Unit).
- `blobs[].file` is relative to the manifest's folder; `bytes` and `fnv1a64` (FNV-1a 64 as 16 hex digits) let a test notice a damaged copy.
- `params[].normalized` is set for VST3 only. `notes` (per state and for the capture) lists what could not be done.

Bytes are saved as the plugin wrote them, with no interpretation. A blob can hold a host-side wrapper the plugin did not write: for example, the Audio Unit dictionary has the keys the Audio Unit base class always writes (`version`, `type`, `subtype`, `manufacturer`, `name`, `data`) around whatever the plugin adds.

### Reading the old format: `openplugin_core::foreign_state`

The module has readers for the containers old formats are made of. Each takes untrusted bytes and returns an error, never a panic, and checks every declared length against what is really there before allocating. `Limits` bounds nesting depth (default 64), node count (2^20), input and single-blob size (64 MiB) and the total size built (128 MiB).

| Module | Reads |
|---|---|
| `bytes::Reader` | A cursor over little- or big-endian data: integers of any width, floats, length-prefixed bytes and strings, zero-terminated strings, UTF-16, `check_count` for sizing a vector before it is built. A refused field consumes nothing. |
| `kv` | A run of `key, value` records, each with a length prefix of a chosen width and byte order, with or without a leading record count. Typed getters over the values. |
| `chunk` | One block of `magic, [version], size, payload` with the size counting the header or not (`read_chunk`, `read_exact`), and a sequence of `tag, size, payload` chunks (`TaggedChunks`, with optional even padding). |
| `xml` | One root element, nested elements, attributes, text, CDATA, comments, the five entities and numeric character references. Strict: no DTD or `<!DOCTYPE>` (so no custom entities), UTF-8 only, no duplicate attributes, no control characters, no unclosed or mismatched tags. The parse is iterative, so depth costs no stack. |
| `plist` | Apple property lists: the XML form and `bplist00` (null, booleans, integers up to 128 bits, reals, dates, data, ASCII and UTF-16 strings, UIDs, arrays, sets, dictionaries). `Value` has getters (`get`, `as_str`, `as_data`, ...). A binary list is checked first (trailer, offset table, every offset and reference); shared or cyclic references are followed with a depth limit and a node and byte budget, so a few hundred bytes cannot expand to gigabytes. Apple's own XML plists carry a `<!DOCTYPE>`, which the strict XML reader refuses: remove that line before parsing, as the module tests do. |
| `import` | The pattern below. |

### The `load_state` pattern

`Plugin` is unchanged. The port's `load_state` first asks whether the bytes are a format it knows from the old plugin; if so it decodes, maps, validates, and only then sets anything; if not it reads its own format.

1. Run `capture` on the old plugin and look at the blobs. Find what tells each format and version apart (a magic, a version field, a root element, a plist key).
2. For each, write a `StateFormat { name, detect, decode }`. `detect(&[u8]) -> Option<u32>` returns the format version if the bytes are that format (cheap: look at the front); `decode(&[u8], version) -> Result<ImportedState>` reads them with the readers above into the old plugin's own parameter ids (`ForeignId::Index(u32)` for a numeric id, `ForeignId::Name(String)` for a key) and values as stored, tagged plain or normalised, plus an opaque `chunk` if the port interprets more itself.
3. Build a `ParamMap` from those ids (the scanner manifest's `params[].id`) to your `ParamInfo::id`s: `map(from, "gain")`, or `map_with(from, "cutoff", convert)` where the units differ. `ParamMap::apply(&PARAMS, &state)` returns the values, clamped to your ranges, as table indices, and the old ids it could not place (`unmapped`); a normalised value with no `convert` follows your parameter's own taper.
4. In `load_state`, call `import(&FORMATS, data)`: `None` means no format recognised it (use your own format), `Some(Err(_))` means it was recognised but damaged (refuse it; do not fall back), `Some(Ok(state))` is the decoded state. Decide your policy on `unmapped` (the test refuses a state with any), and set nothing until the whole state is accepted: a refused `load_state` must change nothing.

`crates/openplugin-core/src/foreign_state/import.rs` has a short example that runs as a doctest. The full worked version is in `crates/openplugin-port/tests/state_import.rs`: a "port" whose parameters are in the opposite order of the original's reads a plane of `f32` values (CLAP), a VST3 component state (normalised `f64`s and a trailer) and an Audio Unit property list whose `data` holds big-endian `(id, value)` pairs, and falls back to its own format.

### What the proof covers

- `crates/openplugin-core/tests/foreign_state.rs` (17 tests) reads hand-written fixtures in every container and refuses hostile input without a panic: every truncation of a good XML document, `bplist00` document, key/value run and chunk; invalid UTF-8 and UTF-16; XML nested 200,000 deep (refused by the depth limit, no stack overflow) and a `<!DOCTYPE>` entity bomb; a binary plist array declaring 2^40 entries, data and strings declaring 4 GiB, a self-referencing array, and a 1 MiB blob referenced 20,000 times (refused by the byte budget); bad offsets, references and trailer sizes; a key/value count of 4 billion; chunk sizes of 2^32 and 2^64 - 1; and 60,000 random mutations of three valid documents.
- `crates/openplugin-port/tests/state_import.rs` captures the SDK's own example. The CLAP capture always runs (the example's `cdylib` is built by the test); VST3 and Audio Unit run when `OPENPLUGIN_PORT_FIXTURES` names the folder of built bundles, as for the scanner. In each format, the default state and a `--set gain=-6 --set Warm=1` state are captured, the blob checksums and sizes match the manifest, the test port loads every blob, and each parameter equals the plain value the capture read back (to 1e-6 for CLAP and Audio Unit, 1e-5 for VST3). The port's parameter table is in the other order, and the scanner manifest's ids map onto it by name. Truncations, single-byte corruptions and extensions of each blob never panic, and a refused load leaves the port's parameters as they were. The Audio Unit blob is read with the SDK's own plist reader, and its `type` and `subtype` equal `aufx` and `ExGn`.
- The CLAP preset-discovery path is tested against a provider built for the test (`crates/openplugin-port/tests/fixtures/preset_provider.rs`, compiled with `rustc`): factory presets at the plugin location, file presets found by walking a folder (with a nested folder and a file of the wrong extension ignored), a preset listed for another plugin id left out, `--max-presets` noting what it dropped. The fixture declares the preset-discovery structures a second time from the CLAP headers, so it checks the code against a second reading of them, not against a third-party provider.
- Read-only smoke over plugins installed on the development machine (not named here): on 15 November 2026, `capture` loaded 30 CLAP bundles (none has a preset-discovery factory, so default state only), 20 VST3 bundles (two have program lists with a program-change parameter: 11 states each, with distinct parameter vectors; the rest have no program list, or a list with no program-change parameter or program data) and 41 Audio Unit components (all captured; 40 have factory presets, 27 of them just one, the rest up to the 20 asked for). All 259 Audio Unit ClassInfo blobs read with the plist reader (`OPENPLUGIN_PORT_SMOKE_DIR`, test `captured_class_info_blobs_of_other_plugins_read_with_the_plist_reader`).

### Limits of the capture

- State only: audio, editors and the plugin's behaviour are not captured. Where a format's save depends on the plugin being played first (a plugin that builds state from audio), the state is as at load.
- VST3 programs are selected the way hosts do it, through a program-change parameter. A plugin that exposes programs only through `IUnitInfo::setUnitProgramData`, or whose programs the component applies only on a later block, may give states that equal the default; the manifest's `params` shows it (equal vectors). `IProgramListData` data is saved when offered, not interpreted.
- CLAP presets need both a preset-discovery factory and the plugin's `clap.preset-load/2`; a bundle with only the first is noted. A provider that lists presets only through a `file` location it expects the host to scan from a path it chose itself is read from the locations it declares.
- A preset that needs a licence, a download or a dialog is skipped with a note. Presets that load into only a part of the state (a layer of a multi-timbral instrument) save the whole state afterwards.
- Preset names and locations come from the plugin's own provider and are written to the manifest as given.

## Limits of the scanner

- A plugin that needs a licence, a dongle or a login at load time may fail to scan or may show a dialog; the timeout bounds the wait.
- Parameters that appear only after the plugin has been configured (a plugin that builds its parameter list from a loaded file) are seen as they are at load.
- Value strings are what the plugin prints; a plugin that prints a rounded or abbreviated value can defeat the taper fit, in which case the comment says `guessed` and the result is `Linear`.
- Audio Unit parameters that are not in the global scope are not listed.
- AAX parameters, ranges, buses and latency are not read: the plug-in is not hosted.
