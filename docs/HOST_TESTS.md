# Host smoke tests

The example plug-in (`examples/editor`, "Example Gain": Gain in dB from -24 to 24, and Warm on/off) is loaded in two real hosts, in every format the hosts support, by scripts that need no one at the keyboard.

| Host | Formats | What runs |
|---|---|---|
| REAPER | CLAP, VST3, Audio Unit | A ReaScript (`reaper_smoke.lua`) in a separate REAPER instance with its own configuration |
| OpenGrid9 (Ardour based) | VST3, Audio Unit, LV2 | A Lua worker (`og9_worker.lua`) in the headless backend `opengrid9-lua` |
| `auval` | Audio Unit | `auval -v` and `auval -strict -v` on `aufx ExGn OpGd` |

OpenGrid9 has no CLAP host and REAPER's LV2 support is not part of this task, so those two cells do not exist.

## Run it

Build the bundles ([BUILD.md](BUILD.md)), then run the one entry point:

```bash
scripts/build_plugin.sh --manifest-path examples/editor/Cargo.toml --version 0.3.1 \
  --name "Example Gain" --bundle-id com.opengrid.example --manufacturer OpenGrid \
  --au-subtype ExGn --au-manufacturer OpGd --ttl-bin lv2_ttl --editor --tail \
  --formats clap,lv2,vst3,au --vst3-sdk /path/to/vst3sdk --au-sdk /path/to/AudioUnitSDK \
  --out build/host-smoke/out --work-dir build/host-smoke/work

python3 scripts/host_smoke/run.py --build build/host-smoke/out --work-root /tmp/openplugin-host-smoke --install-au
```

Options: `--hosts reaper,og9`, `--formats clap,vst3,au,lv2`, `--work-root DIR` (no space in the path). Without `--install-au` the Audio Unit is skipped: it is the only format that needs an install. The exit code is 0 only when every check passed. `report.json` in the work root holds every check with its evidence, and the renders, projects and sessions stay there.

Needs: REAPER in `/Applications/REAPER.app`, OpenGrid9 in `/Applications/OpenGrid9.app`, Python 3. A fresh REAPER configuration asks once whether to choose an audio device; the runner answers No for its own REAPER process through System Events, which needs Accessibility access for the terminal. REAPER's first start with a configuration scans every plug-in on the machine (over 30 minutes on a machine with a large plug-in collection); the scan is cached in the work root, so later runs start in seconds.

## What is touched

- **Nothing is installed for CLAP, VST3 and LV2.** REAPER's configuration (`reaper_cfg/reaper.ini` in the work root) lists the build folder as its CLAP and VST3 search path. OpenGrid9 runs with `HOME` set to a folder in the work root; that folder holds symlinks to the VST3 and LV2 bundles, and the caches of OpenGrid9's own scanners (`ardour-vst3-scanner`, `ardour-au-scanner`). The headless backend only knows plug-ins in the default folders and in those caches, and it ignores `VST3_PATH` and `LV2_PATH`.
- **The Audio Unit is installed for the run and removed again.** `--install-au` copies `Example Gain.component` into `~/Library/Audio/Plug-Ins/Components` (never a system folder, no `sudo`), refuses to start if a component of that name is already there, restarts `AudioComponentRegistrar`, waits until `auval -a` lists `aufx ExGn OpGd`, and removes exactly that path in a `finally` block, even when a check fails.
- **A separate REAPER instance** (`-newinst`, `-cfgfile`) started by the runner, never an existing one. The runner stops only the process it started, and only after a ten minute timeout.
- **The script starts REAPER through `Scripts/__startup.lua`** of that configuration, because a script given on REAPER 7.80's command line did not run in this setup. The runner writes the file for the run and deletes it afterwards.

Safety rules in the code, written after a script elsewhere deleted part of a disk through an unquoted path with a space:

- No command runs through a shell; every command is an argument list.
- Every deletion goes through `safe_remove`, which refuses an empty path, `/`, the work root itself and anything outside the work root. The Audio Unit removal has its own exact-path guard.
- The work root must not contain a space.
- The Lua workers never delete.

## The checks

**REAPER** (per format, one run each):

1. a track with a 3 s stereo sine (1 kHz, 0.25 peak) is made; the plug-in is inserted by name
2. the parameters Gain and Warm exist
3. Gain is set to 0, +6, -12 and +12 dB and the project is rendered each time through the plug-in; the peak of the file must match the gain (4 % tolerance)
4. Warm on at +12 dB: the peak must be `tanh(0.25 * 3.98) = 0.7596` (the example's saturator)
5. the value read back after a render is the value set
6. static values are saved, the project is reopened, Gain and Warm must be the same
7. an envelope on Gain from 0 to +12 dB over the clip is written, rendered, saved, reopened: two points with the same values, and the render of the reopened project must ramp (last tenth over first tenth of the file, about 3.5)
8. the editor window is opened and closed twice (`TrackFX_Show`, then `TrackFX_GetFloatingWindow` must report a window, then none)

**OpenGrid9** (per format): create a session, add the plug-in, list its parameters, sweep Gain over its range in nine steps and read each back, turn Warm on and off, set Gain to +9 dB and Warm on, wait for the engine to run some cycles, save, reopen in a new process and compare both values.

**auval**: `auval -v` and `auval -strict -v` on `aufx ExGn OpGd`.

## Results

15 November 2026, macOS 15.7.5, Apple silicon, bundles built `--archs arm64`, ad-hoc signed, `--editor --tail`.

Hosts: REAPER 7.80/macOS-arm64 (rev 9d9fa7, 13 September 2026, evaluation licence); OpenGrid9 build 9.8.294 (headless Lua backend).

| Check | REAPER CLAP | REAPER VST3 | REAPER AU | OG9 VST3 | OG9 AU | OG9 LV2 |
|---|---|---|---|---|---|---|
| Loads, finds Gain and Warm | pass | pass | pass | pass | pass | pass |
| Gain levels 0, +6, -12, +12 dB in a render | pass | pass | pass | not run | not run | not run |
| Warm (tanh) in a render | pass | pass | pass | not run | not run | not run |
| Set values read back | pass | pass | pass | pass | pass | pass |
| Save, reopen, values kept | pass | pass | pass | pass | pass | pass |
| Gain envelope: kept, audible after reopen | pass | pass | pass | not run | not run | not run |
| Editor opens and closes (twice) | pass | pass | pass | not run | not run | not run |
| Gain text shows the value in dB | pass | **fail** | pass | not run | not run | not run |

`auval -v aufx ExGn OpGd`: AU VALIDATION SUCCEEDED. `auval -strict -v aufx ExGn OpGd`: AU VALIDATION SUCCEEDED.

105 checks passed and one failed in the last full run (the VST3 text, below). The levels are exact: for example the +6 dB render peaks at 0.4988 (expected 0.4988) and Warm at +12 dB at 0.7596 (expected 0.7596) in all three formats. Parameters seen: REAPER lists Gain, Warm, Bypass, Wet and Delta for CLAP and the Audio Unit, and Gain, Warm, Bypass (the VST3 bypass parameter), REAPER's own Bypass, Wet and Delta for VST3. OpenGrid9 lists Gain, Warm and Bypass for VST3, Gain and Warm for the Audio Unit, and four audio ports then Gain and Warm for LV2.

### Findings

1. **VST3 shows Gain as the normalized value, not in dB (SDK bug, not fixed here).** `ShellParameter` in `native/vst3/OpenPluginVST3.cpp` overrides `toPlain` and `toNormalized` but not `toString`/`fromString`. The SDK's `Parameter::toString` prints the normalized value (`printFloat(normValue, precision)`, `public.sdk/source/vst/vstparameters.cpp`), so a host that asks the controller for a parameter's text gets `0.6250` for +6 dB. Reproduction: build the VST3, insert it in REAPER and call `reaper.TrackFX_FormatParamValueNormalized(track, fx, 0, 0.625, "")`; it returns `0.6250 dB` (the unit is added by REAPER). CLAP returns `6.0 dB` and the Audio Unit `6.0000` for the same call. The ends of the range read `0.0000` and `1.0000` instead of `-24.0` and `24.0`. `fromString` has the same gap, so a value typed into the host is read as a normalized one. The fix is to override `toString` and `fromString` in `ShellParameter` through `op_shell_to_plain`, or to ask the plug-in's `ParamSpec::format`.
2. **The plug-in is called "Editor Gain" in CLAP, VST3 and LV2 and "Example Gain" in the Audio Unit.** The first three read the plug-in's own name from `examples/editor`; the Audio Unit takes the product name given to `build_plugin.sh --name`. It is not a defect, but a user sees two names, and the scripts have to look for both.
3. **One intermittent silent VST3 render in REAPER.** In one of six REAPER VST3 runs, four consecutive renders after the first (Gain +6, -12, +12 dB and Warm) came out exactly silent; the later renders of the reopened project were normal, and the five other runs, including the last, were clean. It was not reproduced (two runs straight after it and the final full run passed), so it is recorded without a cause: it may be REAPER's offline render, and it may be a plug-in or shell state problem. A reproduction needs a loop of `reaper_smoke.lua` runs on VST3 with the renders kept; `render_VST3_g6.wav` is the file to look at.

### Observed host behaviour, not defects

- **OpenGrid9 VST3 state needs the engine to run.** The VST3 component state is the processor's, which learns a parameter change in `process`. A script that sets a value and saves at once saves the old value; the worker waits 1.5 s so the engine runs some cycles first. The same holds in REAPER: a value set and saved with no render in between is the old one, which is why the REAPER script renders before it saves.
- **REAPER hands a CLAP parameter to the plug-in at its next process.** `TrackFX_GetParamNormalized` right after a set returns the old value, and the formatted text stays at the old value, until a render or playback has run.
- **REAPER envelopes for CLAP and the Audio Unit speak the parameter's real range (-24 to 24), for VST3 the normalized range (0 to 1).**
- **OpenGrid9 headless can crash in `Route::add_processors` when the plug-in is not in its plug-in list.** `new_plugin` then returns a null plug-in, and adding it segfaults the process. The worker checks `isnil()` first and reports "not found". OpenGrid9's own `LuaAPI.get/set_plugin_insert_param` index control inputs only, so for LV2 (audio ports first) the worker maps the port index to its control ordinal.

## Not covered

- Audio through the plug-in in OpenGrid9: the headless backend renders no signal through inserts (see OpenGrid9's own notes), so OpenGrid9 proves loading, parameters and session state, not level. REAPER proves level.
- The editor's pixels. The window opens and closes, in a second pass too; whether it draws is proven by the in-process editor proofs, and a screenshot is not part of the run.
- CLAP and LV2 in the other host, AAX, Logic, Cubase, Bitwig and Pro Tools, Intel hardware, and any host on Windows or Linux.
- A REAPER run with playback on a real audio device: the runner answers No to the device prompt, so REAPER renders offline.
