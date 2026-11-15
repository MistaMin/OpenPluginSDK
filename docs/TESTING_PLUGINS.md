# Testing your plugin

`openplugin-testing` runs your own `Plugin` through what hosts do, and measures what it costs. It is a
dev-dependency: nothing from it ends up in the plugin you ship.

```toml
[dev-dependencies]
openplugin-testing = "0.3"
# or, through the facade:  openplugin = { version = "0.3", features = ["testing"] }  ->  openplugin::testing
```

## Conformance in one call

Put this in `tests/conformance.rs` of your plugin crate (or under `#[cfg(test)]` in `main.rs`):

```rust
use openplugin_testing::{conformance, Options};

// One line: it installs the counting allocator that the allocation check needs.
openplugin_testing::install_allocator!();

#[test]
fn my_plugin_conforms() {
    conformance(MyPlugin::default, Options::default()).assert_pass();
}
```

`conformance(make, options)` builds fresh plugins from `make` whenever it needs one, runs everything below,
and returns a `Report`. `assert_pass()` panics with the whole report when any check failed. In a debug
build use `Options::quick()` (3 rates, 5 block sizes); the full run takes a few seconds with `--release`.

`install_allocator!()` declares a `#[global_allocator]`, and a binary has only one. If your test binary
already has one, do not use the macro; without it the allocation check is reported as skipped, and the
rest runs.

### What it does

- Sample rates 22050, 44100, 48000, 88200, 96000, 176400, 192000 and 384000. A rate the plugin refuses
  through `accepts_sample_rate` is skipped and listed in the report.
- Block sizes 1, 2, 3, 7, 16, 64, 333, 512, 1024, 4096 and 8192, each as the `prepare` maximum, plus a
  random sequence of block sizes inside one maximum.
- Input: silence, DC, full-scale noise, a sine sweep, bursts of NaN and plus and minus infinity, and
  denormal-range values.
- Parameter fuzz: random values, out-of-range ones, NaN and infinity, at random sample offsets (the block
  is split at each write, like a host does), for every parameter; and every parameter alone at a list of
  extreme values. An id that is not a parameter must be refused.
- `reset` in the middle of a tail; state round-trips; every layout in `supported_layouts` (with a
  sidechain where the layout has one, through `process_buses`; `process` and `process_buses` alternate
  otherwise); offline on and off; transport changes, including malformed ones; notes, MIDI and
  `all_notes_off` when `note_ports().input` is set, and `pull_midi` when it has an output.

### The checks

| Check | Passes when |
| --- | --- |
| `no_alloc` | `process`, `process_buses`, `set_param`, `set_transport`, note and MIDI calls, `pull_midi` and `reset` make no allocation and no free. |
| `no_panic` | No call panics, with hostile parameters, transport, MIDI and state bytes (panics are caught). |
| `finite_output` | Finite input gives finite output. |
| `no_denormals` | No subnormal number appears in the output, including while a tail decays. |
| `bounded_output` | Output stays under `Options::max_output` (default 64). |
| `hostile_input` | After NaN and infinity bursts, output is finite again within one second plus latency plus tail. Passing NaN through is allowed (set `strict_hostile_output` to forbid it); a filter that keeps the NaN in its state is not. |
| `param_fuzz` | `set_param` returns true for every parameter, `param` stays finite and inside `min..max` whatever was written. |
| `latency` | `latency()` does not change while processing (unless `latency_depends_on_params`), does not depend on the `prepare` block size, and is under ten seconds. With `verify_latency_by_impulse` an impulse must peak at the reported latency, for plugins where that is meaningful. |
| `tail` | After input stops, output is below `silence_db` (default -90 dBFS) once latency + reported tail (+ `tail_slack_ms`, default 10 ms) have passed. Skipped for `Tail::Unknown` and `Tail::Infinite`. |
| `reset` | After `reset` in the middle of a tail, silent input gives silent output. |
| `state` | Writing back what `param()` returned changes nothing; `encode_params`, `decode_params` and `restore_params` restore every parameter bit for bit; the plugin's chunk loads into a fresh instance (`load_state` returns true), saves again byte for byte, and leaves the parameters unchanged; `state_is_parameters_only` plugins save no chunk; truncated and random bytes passed to `load_state` do not panic or break processing. |
| `layouts` | Every layout the plugin lists is accepted by `set_layout`, prepares and processes. |
| `locks` | Always skipped. See below. |

### Reading the report

```text
conformance: My Plugin (seed 0x000000000be05eed, 228 sessions)
  [pass] no_alloc        no allocation or free on the audio path (436256 assertions)
  [FAIL] hostile_input   recovers from NaN and infinity at the input (8606 assertions, 8606 failed)
         first failure: output is NaN at channel 0, sample 0 of a 512-frame block [after 34816 frames, block 68]
         case: rates rate=44100 max_block=4096 layout=default offline=false stimulus=nan_inf_bursts plan=fixed512
         reproduce: seed 0x000000000be05eed (case seed 0xc42b15dd5e742df3)
  [skip] locks           no locks on the audio path (0 assertions)
  result: FAIL
```

Every check is one line: `pass`, `FAIL` or `skip`, and how many assertions it evaluated. A failed check
shows its **first** failure: what went wrong, how far into the case, and the `case` text, which names the
rate, block size, layout, stimulus and block plan. The run is deterministic: it uses no clock and no OS
randomness, only the seed (`Options::seed`, or the environment variable `OPENPLUGIN_TEST_SEED`, decimal or
`0x` hex). The same seed on the same plugin gives the same report. To rerun just one failing case, set
`Options::filter` to the `case` text (any part of it); the case's own random stream is derived from the
seed and its text, so it replays the same way.

A skipped check has a `note:` line saying why. In programmatic use, `Report::status(Check::Tail)`,
`Report::failed_checks()` and `Report::result(check).first_failure` give the same information.

### Options worth knowing

`max_output`, `silence_db`, `tail_slack_ms`, `max_tail_seconds`, `latency_depends_on_params`,
`verify_latency_by_impulse`, `strict_hostile_output`, `strict_denormals`, `skip` (leave checks out; they
are reported as skipped), `sample_rates`, `block_sizes`, `fuzz_sessions`.

### What it does not check

**Locks.** A mutex on the audio path cannot be detected reliably from user space: the kit would have to
interpose on the platform's lock primitives for code it does not control, and a lock that is never
contended costs nothing and looks like nothing. The `locks` check therefore always reports `skip`. Use the
thread sanitizer, or a profiler that shows time in `pthread_mutex_lock`, for that.

Allocation is counted for the calling thread only, so a plugin that hands work to its own thread is not
seen. A first-block allocation after `prepare` is counted, because it is on the audio path.

## Benchmarks

`bench` measures the CPU time of `process` per block as a fraction of the block's real time (`load =
processing time / (frames / sample_rate)`), across sample rates, block sizes and channel counts. It warms
up, times each block on its own (the timer's own cost is measured and subtracted), and reports the median,
the 99th percentile, the mean and the extremes.

```rust
use openplugin_testing::bench::{bench, BenchOptions};

let report = bench(MyPlugin::default, &BenchOptions::default());
print!("{}", report.to_text());
```

By default it runs 44.1, 48 and 96 kHz, blocks of 32, 64, 256, 512 and 1024 frames, and each layout the
plugin lists; `channel_counts: Some(vec![1, 2, 6])` runs plain `prepare` with those counts instead. The
input is noise at -12 dBFS; an instrument is given one held note. **Use `--release`**: the report says
whether it was a debug build, and `compare` warns when a debug report is compared with a release one.

```text
bench: OpenPlugin Gain (2000 blocks per case, release build)
rate        block  ch layout           median %      p99 %      max %
48000          64   2 Stereo             0.0219     0.0313     0.0343
48000         512   2 Stereo             0.0535     0.0574     0.2352
```

`median %` is a percentage of real time: 0.05 means the block took 0.05 % of the time it lasts, so 2000
instances would fill one core. The p99 shows what a bad block looks like; `max` is mostly the operating
system.

### Comparing two commits

`report.to_json()` writes the same report with a fixed key order and fixed precision, so it diffs cleanly,
and `BenchReport::from_json` reads it back. `compare(&before, &after, 0.25)` matches cases by rate, block
size and channel count, and flags a case as slower when its median grew by more than the tolerance and by
more than a small absolute floor.

A runner for your plugin is a four-line `main`, using the command-line front end:

```rust
fn main() -> std::process::ExitCode {
    openplugin_testing::bench::cli(MyPlugin::default, std::env::args().skip(1))
}
```

```sh
cargo run --release --example bench -- --out before.json     # on the old commit
cargo run --release --example bench -- --baseline before.json # on the new one; exit 1 if slower
```

Flags: `--json`, `--out FILE`, `--baseline FILE`, `--tolerance 0.25`, `--quick`, `--filter TEXT`,
`--iterations N`. Timing on a busy machine is noisy; leave the tolerance at 25 % or more, run on a quiet
machine, and rerun a flagged case before believing it. The SDK's examples have a runner:

```sh
cargo run --release -p openplugin-testing --example bench_examples -- gain
```
