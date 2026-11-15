# Format parity

One plugin, one input, one automation, six renderings: the Rust plugin called directly (the reference), and the built CLAP, LV2, VST3, Audio Unit and AAX entries. Every output must equal the reference sample for sample. The test is `scripts/format_parity/run.sh`.

```bash
VST3_SDK=/path/to/vst3sdk AU_SDK=/path/to/AudioUnitSDK \
  [AAX_SDK=/path/to/aax-sdk] scripts/format_parity/run.sh [--work DIR] [--tolerance T] [--skip-build] [--jobs N]
```

Every path is quoted, so the repository and `--work` may contain spaces. A format whose SDK variable is unset is skipped and printed as skipped; CLAP and LV2 need only Rust. Nothing is installed: the bundles are loaded from `--work` by path, the Audio Unit is registered in the test process only (`AudioComponentRegister`, as `au_proof` does), and the script deletes only the output files it wrote under `--work/out`.

## What runs

| Entry | Host | Loads |
|---|---|---|
| reference | `parity-host reference` (Rust) | `EditorGain` from `examples/editor` through the `Plugin` trait: `prepare`, `set_param`, `process`. No adapter. |
| CLAP | `parity-host clap` (Rust, `dlopen`) | `Editor Gain.clap/Contents/MacOS/Editor Gain`: `clap_entry`, the factory, `clap_plugin.process` with parameter events |
| LV2 | `parity-host lv2` (Rust, `dlopen`) | `EditorGain.lv2/EditorGain.dylib`: `lv2_descriptor`, `connect_port`, `run` |
| VST3 | `ParityVST3` (C++) | `Example Gain.vst3` through the VST3 hosting interfaces: `IAudioProcessor::process` with `IParameterChanges` |
| Audio Unit | `ParityAU` (C++) | `Example Gain.component` through AudioToolbox: `AudioUnitScheduleParameters` (immediate events at their offsets), `AudioUnitRender` |
| AAX | `ParityAAX` (C++) | the shell's process callback `openplugin_aax::process` with a controls packet, as `aax_process_proof` calls it. Not Pro Tools. |

The CLAP and LV2 bundles are the example's `cdylib` copied into the layout `scripts/build_plugin.sh` uses. The C++ hosts use only their format's API, never `op_shell_*`: parameters are found by name and converted by the format (the VST3 controller's `plainParamToNormalized`). `examples/gain` has no format exports (it is a binary), so `examples/editor`, which has the same gain and exports every format, is the plugin under test.

The Rust host (`scripts/format_parity/host`) is its own Cargo workspace, outside `cargo test --workspace` and the published crates. It writes the scenario file that every host reads and the output files that `parity-host compare` checks, so no host generates a signal of its own and no two languages need to agree on a `sin`.

## Scenario

4096 frames at 48 kHz in host blocks of 512, stereo. The input is a 997 Hz tone on the left and a 1523 Hz tone on the right (peaks 0.45 and 0.3) plus 0.1 of a fixed-seed noise, so no two samples repeat. Automation, in plain values (frame, parameter, value):

| Frame | Offset in its block | Change |
|---|---|---|
| 0 | 0 | Gain -3 dB, Warm 0 |
| 700 | 188 | Gain +6 dB (mid-block) |
| 1800 | 264 | Warm 1 (mid-block, `tanh` on) |
| 2048 | 0 | Gain +2 dB (a block edge) |
| 2600 | 40 | Gain -9.5 dB (mid-block) |
| 3300 | 228 | Gain +12 dB and Warm 0 (mid-block, two parameters at once) |

The reference peak is 2.163.

## Where a change takes effect

The same automation is applied to every format the way that format's contract allows:

- **CLAP, VST3 and the Audio Unit** are sample-accurate. The host puts each change at its offset in the block (a CLAP `param_value` event with that `time`, a VST3 queue point with that `sampleOffset`, an Audio Unit immediate event of `AudioUnitScheduleParameters` with that `bufferOffset`), and the shell splits the block there.
- **LV2 and AAX** apply a parameter value once per `run` or process callback (an LV2 control port is read at the start of `run`; AAX posts one controls packet per callback). For these the host cuts the block at every change frame and sets the value before the sub-block that starts there. The sample positions of the changes are identical; only the way the host expresses them differs. This is the format's contract, not a loosened tolerance. A host that sends one value per 512-frame block to these formats gets block-rate automation by that contract, and this test does not claim otherwise.

The reference applies each change at its frame, so all six are comparing the same function of time.

## Tolerance

The tolerance is an absolute sample difference of **1e-6** (`--tolerance`). The plugin does one multiply and an optional `tanh` per sample, in `f32`, and every format runs that same Rust code, so the only legitimate source of a difference is a parameter value that reached the engine through a different path: VST3 converts plain to normalized to plain in `f32`, whose relative step is 6e-8; the reference peak is 2.163, where one `f32` ulp is 2.4e-7. 1e-6 is about four ulp at the peak, enough to absorb that conversion and not enough to hide a perturbation of 5e-7 of the signal. The measured difference for every format is exactly 0.

## Negative controls

A comparison that cannot fail proves nothing, so the run also requires failures that must be caught:

1. **Altered shells (VST3, Audio Unit, AAX).** `scripts/format_parity/parity.cmake`, included only by `-DOP_FORMAT_PARITY=ON`, builds each shell's unmodified sources a second time with its process call renamed to a faulty one that runs the real engine and scales the output by `1 + 1e-4` (8.7e-4 dB). Shells that process through `op_shell_process_events` (VST3, the Audio Unit and AAX) are built with `-Dop_shell_process_events=parity_faulty_process_events` (`native/parity_fault_events.cpp` for VST3; `parity_faulty_process_events` in `scripts/format_parity/native/parity_fault.cpp` for the Audio Unit and AAX); the others with `-Dop_shell_process=parity_faulty_process` and `native/parity_fault.cpp`. Nothing in a shipped target changes, and the option is off by default. The test fails these on the same comparison: maximum difference 2.162e-4, 216 times the tolerance.
2. **Late automation.** The reference is rendered again with every change one frame late and compared with the real reference: maximum difference 1.219, so a host or format that applied a change a sample off would fail. This shows the test is sensitive to the sample position of a change, which the gain error above is not.

The run exits non-zero if any parity comparison fails, any host errors, or any control passes.

## CI and local-only parts

The `format-parity` job in `.github/workflows/ci.yml` builds the example, the VST3 and Audio Unit shells and the hosts against the pinned public SDKs, and runs the script with CLAP, LV2, VST3 and Audio Unit. AAX runs locally only: its SDK is not redistributable, like `scripts/aax_validate.sh`. Set `AAX_SDK` to include it.

## Results

On 15 November 2026, Apple silicon, `examples/editor` 0.3.1: CLAP, LV2, VST3, Audio Unit and AAX all matched the reference with a maximum absolute difference of 0, at tolerance 1e-6. The VST3, Audio Unit and AAX fault shells each failed with 2.162e-4, and the one-frame-late reference with 1.219.
