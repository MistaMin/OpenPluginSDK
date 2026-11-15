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

15 November 2026 (second full run, bundles built from main at `ab5846d`, after the shell parameter text calls reached VST3 and the Audio Unit), macOS 15.7.5, Apple silicon, bundles built `--archs arm64`, ad-hoc signed, `--editor --tail`. The Audio Unit was installed for the run by `--install-au` and is gone again (`ls ~/Library/Audio/Plug-Ins/Components | grep -i "example gain"` and `auval -a | grep ExGn` are empty).

Hosts: REAPER 7.80/macOS-arm64 (rev 9d9fa7, 13 September 2026, evaluation licence); OpenGrid9 build 9.8.294 (headless Lua backend); auval from macOS 15.7.5.

| Check | REAPER CLAP | REAPER VST3 | REAPER AU | OG9 VST3 | OG9 AU | OG9 LV2 |
|---|---|---|---|---|---|---|
| Loads, finds Gain and Warm | pass | pass | pass | pass | pass | pass |
| Gain levels 0, +6, -12, +12 dB in a render | pass | pass | pass | not run | not run | not run |
| Warm (tanh) in a render | pass | pass | pass | not run | not run | not run |
| Set values read back | pass | pass | pass | pass | pass | pass |
| Save, reopen, values kept | pass | pass | pass | pass | pass | pass |
| Gain envelope: kept, audible after reopen | pass | pass | pass | not run | not run | not run |
| Editor opens and closes (twice) | pass | pass | pass | not run | not run | not run |
| Gain text shows the value in dB | pass (`6.0 dB`) | pass (`6.0 dB`; see finding 4) | pass (`6.0 dB`) | not run | not run | not run |

`auval -v aufx ExGn OpGd`: **AU VALIDATION FAILED** (exit 255). `auval -strict -v aufx ExGn OpGd`: **AU VALIDATION FAILED**. Both fail on the same single section, a regression since the first run, which succeeded: see finding 5. Every other auval section (open, parameters, render tests, MIDI, connection semantics) passes.

102 checks passed and two failed in the last full run: the two failures are the auval runs (finding 5); all 84 REAPER checks and all 18 OpenGrid9 checks passed, the Gain text check in VST3 included. The levels are exact: for example the +6 dB render peaks at 0.4988 (expected 0.4988) and Warm at +12 dB at 0.7596 (expected 0.7596) in all three formats. Parameters seen: REAPER lists Gain, Warm, Bypass, Wet and Delta for CLAP and the Audio Unit, and Gain, Warm, Bypass (the VST3 bypass parameter), REAPER's own Bypass, Wet and Delta for VST3. OpenGrid9 lists Gain, Warm and Bypass for VST3, Gain and Warm for the Audio Unit, and four audio ports then Gain and Warm for LV2.

### Findings

1. **VST3 showed Gain as the normalized value, not in dB. Fixed for VST3, the Audio Unit and AAX.** `ShellParameter` in `native/vst3/OpenPluginVST3.cpp` overrode `toPlain` and `toNormalized` but not `toString`/`fromString`, and the SDK's `Parameter::toString` prints the normalized value, so a host that asked for the text of +6 dB got `0.6250` (REAPER added the unit: `0.6250 dB`; CLAP gave `6.0 dB`, the Audio Unit `6.0000`), the ends read `0.0000` and `1.0000`, and a typed value was read as a normalized one. The fix is the shell ABI 3 calls `op_shell_param_format` and `op_shell_param_parse` (the plug-in's own `ParamInfo::format` and `parse`; see [SHELL.md](SHELL.md), "Parameter text"), and `ShellParameter::toString`/`fromString` through them. `vst3_proof` now checks that normalized 0.625 on Gain shows `6.0 dB` and that `6 dB` reads as 0.625, and that the toggle round-trips `Off`/`On`. The re-run of 15 November confirms it in REAPER: VST3 Gain set to normalized 0.625 reads `6.0 dB` through `TrackFX_GetFormattedParamValue`. The Audio Unit shell now uses the same two calls: `kAudioUnitProperty_ParameterStringFromValue` and `ParameterValueFromString` return the plugin's text and parse typed text, every parameter carries `kAudioUnitParameterFlag_ValuesHaveStrings`, and choice parameters are indexed with `ParameterValueStrings` (the Audio Unit's `6.0000` is gone; `au_proof` checks `6.0 dB`, `6 dB` reading 6.0 and Warm as `Off`/`On`). In the re-run REAPER shows the Audio Unit's own text too: `6.0 dB` for Gain at 0.625. Readings of Gain at normalized 0 / 0.5 / 0.625 / 1 through `TrackFX_FormatParamValueNormalized`: CLAP `-24.0 dB`, `0.0 dB`, `6.0 dB`, `24.0 dB`; Audio Unit the same; VST3 `-24.0 dB dB`, `0.0 dB dB`, `6.0 dB dB`, `24.0 dB dB` (finding 4). Warm on reads `On` in CLAP and VST3 and `on` in the Audio Unit (REAPER's display of the indexed value string; the plug-in's string is `On`). The AAX shell now uses the same two calls as well: every parameter's `AAX_IDisplayDelegate` (the shell's `ShellDisplay`) calls `op_shell_param_format` and `op_shell_param_parse`, so `GetParameterStringFromValue` returns the plugin's text and `GetParameterValueFromString` reads typed text (the SDK's number display, `6.0000`, is gone), with the host's character limit honoured. `aax_text_proof` checks that Gain at +6 dB (normalized 0.625) reads `6.0 dB`, that `6 dB` reads back as 0.625, that Warm reads `Off`/`On`, and that short limits never exceed the limit; `aax_text_stub_proof` checks a choice, a toggle and multi-byte text. Neither ran inside Pro Tools.
2. **The plug-in is called "Editor Gain" in CLAP, VST3 and LV2 and "Example Gain" in the Audio Unit.** The first three read the plug-in's own name from `examples/editor`; the Audio Unit takes the product name given to `build_plugin.sh --name`. It is not a defect, but a user sees two names, and the scripts have to look for both.
3. **One intermittent silent VST3 render in REAPER.** In one of six REAPER VST3 runs, four consecutive renders after the first (Gain +6, -12, +12 dB and Warm) came out exactly silent; the later renders of the reopened project were normal, and the five other runs, including the last, were clean. It was not reproduced (two runs straight after it and the final full run passed), so it is recorded without a cause: it may be REAPER's offline render, and it may be a plug-in or shell state problem. A reproduction needs a loop of `reaper_smoke.lua` runs on VST3 with the renders kept; `render_VST3_g6.wav` is the file to look at.

4. **VST3 text doubles the unit in REAPER's by-value text call.** `TrackFX_FormatParamValueNormalized` (the text for a value the parameter is not at) returns `6.0 dB dB` for VST3, while `TrackFX_GetFormattedParamValue` returns `6.0 dB`. The plug-in's own text already carries the unit, and `ShellParameter` in `native/vst3/OpenPluginVST3.cpp` is still constructed with a units string (`dB`), which REAPER appends to the text it gets for a value. The Gain text check passes (it looks for `6.0`); the doubled unit is cosmetic. Not changed here; the likely fix is an empty units string for parameters whose text already has the unit.
5. **Fixed (commit `ca83145`; CI's auval job passes since): auval failed after the channel layout work because Binaural was advertised and then refused.** A mono or stereo bus no longer offers UseChannelDescriptions, so auval no longer expects every two-channel layout. The original report: `auval -v aufx ExGn OpGd` (and `-strict`) reports `ERROR: Can not set Channel Layout when should be able to` for tag `0x6A0002` (Binaural, 2 channels), then `Cannot verify Audio Channel Layouts as Format handling has problems`, and ends `AU VALIDATION FAILED`. auval's availability table lists Binaural as available on the 2 channel bus, but `SetAudioChannelLayout` in `native/au/OpenPluginAU.cpp` accepts a tag other than UseChannelDescriptions only if it is in `GetChannelLayoutTags`, which comes from `opau::appendTags` in `native/au/au_layout.h`; for the stereo bus that list is Stereo (`0x650002`) and UseChannelDescriptions, without Binaural, so the set returns `kAudioUnitErr_InvalidPropertyValue`. The code arrived with commit `af0f89c` ("au: parity on shell ABI 2 ..."); the first host run, before it, passed auval. Either `appendTags` should offer Binaural for an L/R bus, or the unit should not report it available. REAPER, OpenGrid9 and all render and parameter checks are unaffected.

### Observed host behaviour, not defects

- **OpenGrid9 VST3 state needs the engine to run.** The VST3 component state is the processor's, which learns a parameter change in `process`. A script that sets a value and saves at once saves the old value; the worker waits 1.5 s so the engine runs some cycles first. The same holds in REAPER: a value set and saved with no render in between is the old one, which is why the REAPER script renders before it saves.
- **REAPER hands a CLAP parameter to the plug-in at its next process.** `TrackFX_GetParamNormalized` right after a set returns the old value, and the formatted text stays at the old value, until a render or playback has run.
- **REAPER's start is slow.** REAPER 7.80 in evaluation shows its "About REAPER" nag at the start of a run, and the script begins only after it is gone, so a run shows no result file for the first 90 s or so. One attempt of this re-run, in a new work root, ended with no result file for any format after the runner's ten minute timeout (REAPER killed by the runner, exit -9); the cause was not established (the session was interrupted), and the next attempt, with the same bundles, ran all three formats.
- **REAPER envelopes for CLAP and the Audio Unit speak the parameter's real range (-24 to 24), for VST3 the normalized range (0 to 1).**
- **OpenGrid9 headless can crash in `Route::add_processors` when the plug-in is not in its plug-in list.** `new_plugin` then returns a null plug-in, and adding it segfaults the process. The worker checks `isnil()` first and reports "not found". OpenGrid9's own `LuaAPI.get/set_plugin_insert_param` index control inputs only, so for LV2 (audio ports first) the worker maps the port index to its control ordinal.

## Pro Tools (AAX), 15 November 2026

Run by hand through computer-use automation, not by `scripts/host_smoke`: Pro Tools Studio 2025.10 on macOS 15.7.5 (arm64), the example built with `scripts/build_plugin.sh --formats aax --editor --tail`, signed with the owner's Developer ID and PACE-signed with `wraptool` (the owner's account, an existing wrap configuration), installed in `/Library/Application Support/Avid/Audio/Plug-Ins`. A session with one stereo audio track, Signal Generator on insert A (a tone at -20.0 dBFS peak) and Editor Gain (stereo) on insert B.

| Check | Result |
|---|---|
| Scan, PACE authorization, plug-in cache | pass: listed as "Editor Gain" by OpenGrid, mono and stereo |
| Insert on a stereo track, audio through it | pass, after the fix below |
| Editor window opens and draws, meter moves | pass |
| Gain by dragging the knob to +9.6 dB | pass: track peak -10.4 dBFS (-20.0 + 9.6), Pro Tools lights COMPARE |
| Warm on at +9.6 dB | pass: peak -10.7 dBFS, as tanh predicts (tanh(0.302) = 0.293) |
| Pro Tools bypass | pass: peak back to -20.0 dBFS |
| Save, close, reopen: Gain and Warm restored | pass: peak -10.7 dBFS again, editor shows 9.6 dB |
| Typing a value (double-click, type, Return) | pass in run 2, after the fix in finding 7 (run 1 failed) |

Findings:

6. **Fixed: the AAX shell faulted on the first block in Pro Tools.** Pro Tools reported "Found stuck plugin com.opengrid.example.editor-gain" and DAE error -9004, and its audio engine wedged until Pro Tools was force-quit. Cause: `process` in `native/aax/OpenPluginAAXProcess.cpp` read `context->midiIn`, `context->midiOut` and `context->sideChain` whenever they were non-null, but Describe registers those fields only when the plugin has note ports or a sidechain, and Pro Tools leaves an unregistered field undefined, not null. The example has no MIDI, so the shell called through a garbage pointer. The shell now reads each optional field only when it registered it, and `aax_process_proof` fills the unregistered fields with faulting addresses as Pro Tools does: the old code segfaults there, the fixed code passes. No fake host before this one could catch it, since they all zeroed the context.
7. **Fixed and verified in Pro Tools (run 2): Return did not commit the type-in field inside Pro Tools.** Double-click opens the field with the formatted value and typing reaches it, but Pro Tools takes Return and Enter for its own commands, so the field never commits (Escape-style cancel by clicking elsewhere works). Pressing Enter in the field may have added a marker to the session. The fix is in two parts (`crates/openplugin-dri/src/cocoa.rs`): while the field is open an `NSEvent` local key-down monitor takes Return, keypad Enter (commit) and Escape (cancel) when the field's editor has the keyboard, before the host's key handling can, and is removed when the field closes, the view leaves its window or the view drops; and losing focus (a click elsewhere, the window resigning key) now commits valid text and discards invalid text instead of cancelling. Proven headlessly with synthetic key events through `-[NSApplication sendEvent:]` (`cargo test -p example-editor --test typing`). Run 2 confirmed it in Pro Tools: Return commits (typing 6 set Gain to 6.0 dB and the peak read -14.1 dBFS, as expected with Warm on), Escape cancels, and clicking elsewhere in the editor commits.


### Run 2 (15 November 2026, the `pro-tools-testing` skill)

A fresh session ("OpenPluginSDK AAX Run 2") with the build that has the AAX fix, then again after the type-in fix.

| Check | Result |
|---|---|
| AAX Validator on the signed bundle | 12 passed, 1 failed, 1 skipped: `test.page_table.load` reports "Failed to load page tables library", the validator's own component (this build has no page tables, `--aax-pages` off); `test.cycle_counts` needs HDX |
| Insert, editor, Gain +9.6 dB, Warm, bypass | pass: -20.0 / -10.4 / -10.7 / -20.0 dBFS |
| Save, close, reopen | pass: -10.7 dBFS after reopening |
| Type-in field: Return, Escape, click elsewhere | pass (finding 7) |
| Stuck plugin or performance errors in the log | none |

Things a run can meet, none of them plugin faults:

- A session in OneDrive: Cmd+S raised "The disk copy of ... was changed since you last opened or saved it. Do you want to save a copy?", because the sync had touched the file. OK leads to Save As; the copy saved and reopened fine.
- After the force-quit in run 1, Pro Tools lost settings: at the next launches it reported "Unable to read the I/O settings file", asked for the Playback Engine (set back to Mac Studio Speakers, the device it used before) and asked again about sending usage data to Avid (`UsageAnalytics.pref` was gone; answered No, changeable in Preferences > Operation > Diagnostics).

## Not covered

- Audio through the plug-in in OpenGrid9: the headless backend renders no signal through inserts (see OpenGrid9's own notes), so OpenGrid9 proves loading, parameters and session state, not level. REAPER proves level.
- The editor's pixels. The window opens and closes, in a second pass too; whether it draws is proven by the in-process editor proofs, and a screenshot is not part of the run.
- CLAP and LV2 in the other host, AAX, Logic, Cubase, Bitwig and Pro Tools, Intel hardware, and any host on Windows or Linux.
- A REAPER run with playback on a real audio device: the runner answers No to the device prompt, so REAPER renders offline.
