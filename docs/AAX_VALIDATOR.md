# AAX Validator, local only

`scripts/aax_validate.sh` runs Avid's AAX Validator against the example AAX bundle. It exists because the validator and the AAX SDK are not redistributable, so no public CI job can run them. Run it on a macOS machine that has both.

## What it runs

1. Unless you pass `--bundle`, it builds `examples/editor` as a staticlib (`cargo build --release --lib -p example-editor`) and the AAX shell alone (`-DOP_BUILD_VST3=OFF -DOP_BUILD_AU=OFF -DOP_AAX_PAGES=ON`, product "Example Gain"). The page table is written into the bundle, so `test.page_table.load` has something to load.
2. It writes a small command file and pipes it into `dsh` (DigiShell): `load_dish aaxval`, then `runtests {path: "<bundle>.aaxplugin", stringformat: yaml}`. The validator loads the plug-in from the path it is given. Nothing is installed into `/Library/Application Support/Avid/Audio/Plug-Ins` or anywhere else, nothing needs sudo, nothing is signed.
3. It reads each `result_status` from the YAML, prints one line per test and a total, and exits 0 only if no test failed. `E_COMPLETED_FAIL`, `E_LOST`, `E_TIMEOUT`, `E_ABORTED` and `E_ERROR` count as failures. Exit 2 means the script could not start (validator or SDK missing, build failed).

The tests are the ones `listtests` reports in validator 2024.6.0: four `info.*` tests (product IDs, general, AudioSuite and S6 support, which report and normally pass) and ten `test.*` tests: `cycle_counts`, `data_model`, `describe_validation`, `load_unload` (1000 loads), `page_table.automation_list`, `page_table.load`, `parameter_traversal.linear`, `parameter_traversal.random`, `parameter_traversal.random.fast`, `parameters`. `test.cycle_counts` measures DSP plug-ins: it acquires an HDX deck through the DAE dish. Without HDX it ends `E_LOST`, so the script reports it as skipped. Set `AAX_VALIDATOR_DSP=1` to count it.

## How to run it

```bash
export AAX_VALIDATOR_DIR=/path/to/aax-validator-dsh-2024-6-0-138bab0d-mac-arm64   # holds CommandLineTools/dsh
export AAX_SDK=/path/to/aax-sdk-2-8-1                                            # holds Interfaces/AAX.h
scripts/aax_validate.sh
scripts/aax_validate.sh --bundle "/path/to/Some Plugin.aaxplugin"   # skip the build
```

Options and variables: `--out DIR` or `AAX_VALIDATOR_OUT` (default `build-aax-validator` in the repository; put it on a volume with space), `AAX_VALIDATOR_TESTS="test.data_model test.parameters"` to run only some tests, `OP_EDITOR=OFF` to build without the editor view, `AAX_VALIDATOR_TIMEOUT` seconds for the whole run (default 1800), `CARGO_BUILD_JOBS` (default 2). Without `AAX_VALIDATOR_DIR` the script prints how to set it and exits 2.

Under the output folder: `aax_validate.log` (build output plus the full dsh output), `aax_validate.raw.log` (dsh output only), `aax_validate.summary.txt` (id and status per test), `aax_validate.dsh` (the commands sent). dsh also writes its own DigiTrace logs into a `Logs` folder next to the `dsh` executable.

## Two problems with the validator itself

Both stopped every test on the machine used for the result below, and both are in Avid's tool, not in the shell.

- **Spaces in the validator path.** dsh starts each test in a child process and passes it the validator's own path unquoted. With `/Volumes/Developer 2/...` every test ends `E_ABORTED` with `ruby: Is a directory -- /Volumes`. A symlink does not help because the path is resolved. The script refuses a `AAX_VALIDATOR_DIR` containing a space; copy the validator (about 1.4 GB without the duplicate nested folder) to a path without one. The bundle path may contain spaces.
- **Mounted volumes without a container size.** `DTT/sources/classes/SysInfo.rb` reads `diskutil info` for every entry in `/Volumes` and calls `[]` on the "Container Free Space" and "Total Size" values. A volume that has neither (exFAT, FAT or a FUSE mount such as a USB stick or a Dropbox mount) makes it raise `undefined method '[]' for nil` and every test aborts. The fix is to eject those volumes, or to change the two lines in your copy to `.to_s[...]`. The script does not edit Avid's files. The result below was made with a copy patched that way, because three such volumes were mounted.

## What it does not do

- It does not run in CI and does not install, sign or authorize anything.
- It does not test inside Pro Tools. The validator checks the AAX contract (description, data model, parameters, page tables, load/unload); audio rendering, the editor view and state in a session are not covered. `aax_process_proof`, `aax_state_proof` and the other AAX proofs in `docs/SHELL.md` cover the callback and state.
- It does not run DSP tests (`test.cycle_counts`) or HDX. The shell is native only.
- It does not check Avid's PACE/iLok wrapping, which a shipped plug-in needs.

## After AAX parity on shell ABI 2

15 November 2026, same validator and machine, with the shell on `op_shell_process_events`, a transport MIDI node and `UsesTransport` in every description: 14 results, 13 passed, 0 failed, 1 skipped (`test.cycle_counts`). The example is an audio effect, so the validator did not see a MIDI node, a sidechain, an instrument or a surround stem; `aax_describe_proof` covers what the shell describes for those, and no AAX validator run does. The copy of the validator was in a path without spaces, and its `SysInfo.rb` patched as above because volumes without a container size were mounted (before the patch three tests ended `E_ABORTED` in `SysInfo.rb:249`, an Avid-tool failure unrelated to the shell).

## Result on this machine

14 November 2026, macOS arm64, AAX Plug-In Validator 2024.6.0 (DigiShell v24.9.0x14), AAX SDK 2.8.1, example "Example Gain" built from `examples/editor` with the editor view and page table: 14 results, 13 passed, 0 failed, 1 skipped (`test.cycle_counts`, needs HDX and the DAE dish). `info.productids` reported manufacturer 1332758372, product 1164199790. The shell needed no change.
