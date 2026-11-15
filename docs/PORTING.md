# Porting toolkit: moving a plugin from another SDK

`openplugin-port` is a set of tools for moving a plugin that already ships in CLAP, VST3, Audio Unit or AAX onto OpenPluginSDK without losing what its users depend on: the same parameters, in the same ranges, with the same identity, so sessions and presets keep working. Each tool answers one question and writes something a person can read and check. None of them edits your plugin or installs anything.

Today the toolkit has one tool, the scanner (porting task 1). It answers "what is this plugin?". The tasks that follow build on its manifest.

Shipping before the DSP is rewritten? [CPP_BRIDGE.md](CPP_BRIDGE.md) shows how to call your existing C++ DSP from a Rust plugin through `cxx` (porting task 7), with the real-time rules and how to test them.

| Task | Tool | State |
|---|---|---|
| 1 | `scan`: read an existing plugin into a manifest and a Rust snippet | this document |
| 2 | Pin identity the SDK derives today (explicit VST3 parameter ids, class ids) | later; the scanner marks each spot with a `TODO(porting task 2)` |

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
  "tool": "openplugin-port 0.3.1-beta",
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

## Limits

- A plugin that needs a licence, a dongle or a login at load time may fail to scan or may show a dialog; the timeout bounds the wait.
- Parameters that appear only after the plugin has been configured (a plugin that builds its parameter list from a loaded file) are seen as they are at load.
- Value strings are what the plugin prints; a plugin that prints a rounded or abbreviated value can defeat the taper fit, in which case the comment says `guessed` and the result is `Linear`.
- Audio Unit parameters that are not in the global scope are not listed.
- AAX parameters, ranges, buses and latency are not read: the plug-in is not hosted.
