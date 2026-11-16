# Stability guide

How to write a plugin that a host cannot break and that cannot break a host, for someone writing a
`Plugin` with this SDK. The companion is [PERFORMANCE.md](PERFORMANCE.md), which is about cost; this
guide is about NaN, blow-ups, panics, threads and the things hosts do that the specification does not
promise they will not do.

The rule of this guide: **every rule links to the test that enforces it in the SDK**, and every
number is measured in [`examples/perf_guide`](../examples/perf_guide). Where a rule has no test in
the SDK, the guide says so under [Findings](#findings-fixed) or says it is not covered.

Two tools carry most of the enforcement, and both are in
[TESTING_PLUGINS.md](TESTING_PLUGINS.md):

- **The conformance kit** (`openplugin_testing::conformance`) runs your plugin through what hosts do:
  every sample rate from 22.05 to 384 kHz, block sizes 1 to 8192, fixed and varying per call, silence,
  DC, full-scale noise, NaN and infinity bursts, parameter fuzz at sample offsets, reset in a tail,
  state round-trips, layouts, offline switching. One call in your plugin's tests.
- **The SDK's own tests**, linked below, which hold the SDK's filters, state gate and format entries
  to the same rules.

[`StableFilter`](../examples/perf_guide/src/stable_filter.rs) is a small worked example that follows
every rule here. [`tests/stable_filter.rs`](../examples/perf_guide/tests/stable_filter.rs) runs the
kit on it and drives by hand the host behaviours the kit does not.

## Numerically stable filters

### Which topology, under modulation

A filter whose coefficients are fixed is stable if its poles are inside the unit circle, and the
textbook forms are all fine. The trouble starts when a cutoff or Q moves while the filter is running
(an LFO, an envelope, an automated knob). The state of a direct-form filter (past inputs and outputs,
or the transposed form's partial sums) was accumulated under the *old* coefficients; change the
coefficients and that state is no longer a state the new filter could have reached, and the filter
rings.

The experiment ([`stability.rs`](../examples/perf_guide/src/stability.rs); run it with
`cargo run --release -p example-perf-guide -- stability`): the same white noise (+-0.5, 48 kHz, 20 s)
through a resonant low-pass whose cutoff moves between 50 Hz and 12 kHz. Six filters: direct form I
(DF1) and transposed direct form II (TDF2), each with `f32` and with `f64` state (coefficients
always computed in `f64`), the SDK's `Biquad` (DF1, `f32`) and the SDK's `Svf` (zero-delay-feedback
state variable filter, `f64` state). The figure is the peak output magnitude; the input is at most
0.5, so a quiet Q-0.7 filter should peak around 0.58 and a Q-10 resonator near its peak gain.

**Cutoff swept by an LFO, coefficients recomputed every sample** (an exponential sweep, 50 Hz to
12 kHz):

| Q | LFO | DF1 `f32` | DF1 `f64` | TDF2 `f32` | TDF2 `f64` | SDK `Biquad` | SDK `Svf` |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0.707 | 5 Hz | 0.578 | 0.578 | 0.578 | 0.578 | 0.578 | 0.578 |
| 10 | 5 Hz | 2.744 | 2.744 | 2.746 | 2.746 | 2.744 | 2.753 |
| 10 | 50 Hz | 7.747 | 7.746 | 8.576 | 8.577 | 7.746 | **2.742** |
| 40 | 50 Hz | 47.13 | 47.13 | 46.77 | 46.77 | 47.13 | **5.760** |

**Cutoff redrawn at random every 16 samples, Q 10** (the worst case, a knob thrown around):

| | DF1 `f32` | DF1 `f64` | TDF2 `f32` | TDF2 `f64` | SDK `Biquad` | SDK `Svf` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| non-finite output samples (of 960 000) | 951 176 | 951 133 | 951 163 | 951 113 | 0 | 0 |
| peak finite output | 1.8e38 | 3.4e38 | 1.8e38 | 3.1e38 | 2.5e38 | **2.83** |

What this says:

- **The state variable filter is the modulation-safe topology.** At Q 40 and a 50 Hz sweep its
  peak is 5.8 where the direct forms reach 47: 8 times the ringing. Under random jumps at Q 10 the
  direct forms went to infinity and stayed (99 % of the output is non-finite), and the SVF's peak
  was 2.8. This is the design property the SDK documents for it: its state variables are
  integrator outputs (energy-like), so changing the coefficients does not inject a transient the way
  it does in a direct form ([`svf.rs`](../crates/openplugin-dsp/src/svf.rs)). Tests:
  `modulation_stress_is_bounded` in [`filters.rs`](../crates/openplugin-dsp/tests/filters.rs) (6 s of
  per-sample modulation and random jumps over 20 Hz to 20 kHz, Q to 20: the `Svf` and the Butterworth
  and crossover cascades must stay below 100, while the matched biquad, a direct-form-family filter,
  is only required to stay finite inside a documented runaway guard of 1e6) and, for these numbers,
  [`tests/stability.rs`](../examples/perf_guide/tests/stability.rs)
  (`the_svf_stays_calm_under_a_fast_resonant_sweep_and_the_direct_forms_ring_harder`,
  `random_cutoff_jumps_at_high_q_blow_up_the_direct_forms_but_not_the_svf`).
- **DF1 against TDF2 is not a stability choice.** Both ring about the same under modulation (TDF2
  slightly more at Q 10 and 50 Hz, 8.58 against 7.75, slightly less at Q 40). Pick between them for
  noise and for the cost of the state, not for stability. A fixed filter that is never modulated can
  be either.
- **`f64` does not cure a modulation problem.** DF1 `f32` and DF1 `f64` peak at the same value to
  three digits (test `state_precision_does_not_change_the_modulation_peak`). Precision and
  topology are different problems; the next section is about precision.
- **The SDK's `Biquad` survives, but only by clearing itself.** Its guard (below) turns a non-finite
  result into a state reset, so it never outputs NaN, but under the random-jump torture the *finite*
  samples before the reset still reach 2.5e38. A plugin that modulates a `Biquad` hard must keep the
  rate of change down (smooth and update per sub-block) or use `Svf`; the kit's `bounded_output`
  check would report the 2.5e38.
- Modulation that is gentle enough (Q 0.7 at 5 Hz) makes no difference at all: all six filters give
  0.578. The topology matters when the modulation is fast or the Q is high, which is also when
  users find the plugin interesting.

Rule of thumb: **a filter that is modulated, or whose Q goes above about 5, is a state variable
filter** (`openplugin_dsp::Svf`, recipe in [DSP_COOKBOOK.md](DSP_COOKBOOK.md#state-variable-filter-modulation-safe)).
A fixed or slowly moving equaliser band is a biquad (`MatchedBiquad` when cramping near Nyquist
matters).

### `f64` state for low cutoffs

A recursive filter's poles sit near 1 when the cutoff is far below the sample rate. At 20 Hz and
192 kHz the poles sit within about `5e-4` of 1; `f32` has a spacing of `6e-8` near 1, and the
coefficients and the state are both quantised to it. The filter's DC gain, which must be exactly 1, is the
canary: a unit step into a low-pass (Q 0.707), the value after 4 s (20 Hz) or 8 s (5 Hz) at
192 kHz:

| Filter | 20 Hz, final value (exact 1) | 5 Hz, final value |
| --- | ---: | ---: |
| DF1, `f32` state | 1.000612 | 0.384 |
| TDF2, `f32` state | 0.998551 | 0.141 |
| SDK `Biquad` (DF1, `f32`) | 0.996103 | 0.99988, see below |
| DF1, `f64` state | 1.000000000 | 1.000000000 |
| TDF2, `f64` state | 1.000000000 | 1.000000000 |
| SDK `Svf` (`f64` state) | 1.000000000 | 1.000000000 |

At 5 Hz an `f32` filter is not a 5 Hz low-pass any more: it settles to 38 % or 14 % of the input.
(The SDK `Biquad` row at 5 Hz reads 0.99988 only because `Biquad::configure` clamps the cutoff to
10 Hz or above; at 20 Hz its `f32` error is 0.4 %.) Test:
`f32_state_cannot_hold_a_very_low_cutoff_and_f64_can` in
[`tests/stability.rs`](../examples/perf_guide/tests/stability.rs).

So: **filters, integrators, phase accumulators and anything that adds a small number to a large
one repeatedly keep `f64` state, and compute coefficients in `f64`.** Buffers and per-sample maps
stay `f32`. It costs nothing in time for a recursion
([PERFORMANCE.md](PERFORMANCE.md#10-f32-or-f64): 35.04 against 35.54 us/block). The SDK's `Svf`,
`MatchedBiquad` (transposed direct form II, `f64` state), `Butterworth`, `Crossover` and the
envelope followers keep `f64` state; `Biquad` is the exception, with `f32` state.

### Smooth what the listener would hear as a step

A parameter that jumps puts a step into the signal. A gain going from 0 to 1 on a 1 kHz sine
changes by 0.866 between two samples; the sine itself never changes by more than 0.131:

| | Largest sample-to-sample change |
| --- | ---: |
| gain stepped | 0.8660 |
| gain through a 10 ms `OnePole` | 0.1303 |
| the sine alone | 0.1306 |

Test: `smoothing_removes_the_step_a_gain_jump_puts_into_the_signal`. Smoothing is also what stops a
coefficient change from kicking a filter: smooth the *cutoff* (and Q), not the coefficients derived
from it, and recompute the coefficients from the smoothed value every 16 to 64 samples (cost and
measurements in [PERFORMANCE.md](PERFORMANCE.md#4-block-rate-work-against-sample-rate-work)).
`StableFilter::process` does exactly that, and `OnePole` itself ignores a non-finite target
(`reaches_the_target_and_ignores_non_finite` in
[`smooth.rs`](../crates/openplugin-core/src/smooth.rs)). Do not smooth what the user expects to be
instant (a bypass or a mode switch: use a short crossfade, `SoftBypass` for bypass).

## Guarding state against NaN, infinity and denormals

A NaN that enters a recursive filter's state stays there: every later output is `NaN`, and a host
that sees NaN may mute the track, show a meter at full scale or, with a bad driver, damage a speaker.
The policy has four parts, and it is the same in every filter the SDK ships:

1. **A non-finite input sample is treated as `0.0` and never reaches the state.** The filter then
   behaves exactly as if it had been given silence for that sample.
2. **A result outside the `f32` range** (which only enormous finite input can produce) **resets the
   state and outputs `0.0`**, so one bad sample cannot silence the filter until the host resets it.
3. **State below `1e-30` is flushed to zero**, so tails reach exact zero and never become
   subnormal (see [PERFORMANCE.md](PERFORMANCE.md#3-denormals-and-flush-to-zero)). The format
   entries also run the plugin with the CPU's flush-to-zero mode on; the flush in code is what
   holds where the mode is off (the kit, a plugin that opts out with `Plugin::flush_denormals`).
4. **A configuration call with a NaN or infinite frequency, Q or gain is ignored** and the previous
   coefficients are kept.

Which SDK filter does which:

| | 1. input | 2. range | 3. flush | 4. bad configure |
| --- | :---: | :---: | :---: | --- |
| `Svf`, `MatchedBiquad`, `Butterworth`, `Crossover`, `OnePoleFilter`, `DcBlocker`, `Allpass1`, `Allpass2` | yes | yes | yes | ignored |
| `EnvelopeFollower` | n/a | n/a | yes (`flush` in [`envelope.rs`](../crates/openplugin-dsp/src/envelope.rs)) | n/a |
| `LongConvolver` | n/a | n/a | yes (stage results and output) | n/a |
| `Dither` shaper | n/a | bounded feedback | yes | n/a |
| `Biquad` | yes | yes | yes | ignored |

Tests (all in [`filters.rs`](../crates/openplugin-dsp/tests/filters.rs) unless noted):
`non_finite_input_does_not_poison_state`, `huge_input_resets_instead_of_overflowing`,
`bad_parameters_are_harmless`, `tails_flush_to_zero_without_denormals` (1 000 000 samples),
`works_at_low_and_high_rates`; for convolution,
[`long_convolution.rs`](../crates/openplugin-dsp/tests/long_convolution.rs). `Biquad`'s guard is
enforced by the kit, through `example_console_eq_conforms` in
[`openplugin-testing/tests/examples.rs`](../crates/openplugin-testing/tests/examples.rs).

`Biquad` guards the signal (a NaN input is silence, a non-finite result clears the state), flushes
its state below 1e-30 like the others, and `configure` ignores a NaN or infinite parameter and keeps
the previous coefficients, as `Svf` does. Test:
`a_nan_cutoff_is_ignored_by_biquad_and_svf` in
[`tests/stability.rs`](../examples/perf_guide/tests/stability.rs), and the unit tests in
[`biquad.rs`](../crates/openplugin-dsp/src/biquad.rs). Clamping a parameter before it reaches
`configure`, as `ParamInfo::clamp` does for anything that comes through `set_param`, is still right.

For your own state, the rule in code (this is what `StableFilter::process` does per sample):

```rust
let x = if s.is_finite() { f64::from(*s) } else { 0.0 };         // 1. input
let y = b0 * x + z1;
let (n1, n2) = (b1 * x - a1 * y + z2, b2 * x - a2 * y);
if !(y.is_finite() && n1.is_finite() && n2.is_finite()) || y.abs() > f64::from(f32::MAX) {
    (z1, z2) = (0.0, 0.0); *s = 0.0; continue;                      // 2. range
}
z1 = if n1.abs() < 1e-30 { 0.0 } else { n1 };                       // 3. flush
z2 = if n2.abs() < 1e-30 { 0.0 } else { n2 };
```

Policy on the *output* of a stage that cannot recover (a table lookup, a `powf`, a division): check
it once where it is computed, per block or per sub-block, not per sample. The kit's `finite_output`
and `bounded_output` checks (`Options::max_output`, default 64) fail on a NaN or an output of
absurd size even if the input was fine. A plugin that passes a NaN input straight through is
accepted by default (`hostile_input` only requires recovery within a second plus latency plus tail);
`strict_hostile_output` demands finite output *during* the burst too.

## Panics never cross the format boundary

A Rust panic that unwinds into a host's C or C++ frames is undefined behaviour in the host, and
since Rust 1.81 an unwind out of an `extern "C"` function aborts the process instead. Either way the
user loses their session. The rule: **a panic never leaves the plugin**; the SDK catches the ones
that escape your code at the boundary it controls, and the kit finds them before you ship.

Where the SDK catches (`std::panic::catch_unwind`):

| Entry | Where | What the host sees |
| --- | --- | --- |
| Native shell ABI used by VST3, Audio Unit and AAX (`export_shell!`): `process`, `midi_out`, `reset`, `save_state`, `load_state`, `create`, `select_layout` | [`openplugin-host/src/shell.rs`](../crates/openplugin-host/src/shell.rs) (`catch_unwind` at each entry) | `process` returns 0 (failure), `save_state` returns `OP_STATE_FAILED`, `reset` returns normally |
| State save and load beside the audio thread | [`openplugin-host/src/lib.rs`](../crates/openplugin-host/src/lib.rs) (`save_bytes` and `load_bytes` wrap the plugin's `save_state` and `load_state`) | `None` or `false`; the gate is released by a guard, so the audio thread is not blocked afterwards |
| The worker thread that builds impulses and tables | [`openplugin-core/src/worker.rs`](../crates/openplugin-core/src/worker.rs) | `Loader::failed` counts the job; the live value stays |

The enforcement in the SDK today:

- **Detection in your plugin: the kit's `no_panic` check.** Every call the kit makes (`process`,
  `process_buses`, `set_param`, `set_transport`, notes and MIDI, `reset`, `load_state` with truncated
  and random bytes) runs under `catch_unwind`; the first panic fails the check with the case that
  caused it and a seed that replays it. Proved on a deliberately panicking plugin:
  `panic_fails_no_panic` and `failure_is_reproducible_from_seed_and_case` in
  [`openplugin-testing/tests/broken.rs`](../crates/openplugin-testing/tests/broken.rs).
- **The state gate holds when a state call fails or times out**: `a_call_gives_up_after_a_bounded_wait_and_changes_nothing`
  in [`shell_state_threads.rs`](../crates/openplugin-host/tests/shell_state_threads.rs).
- **Hostile input at the boundary does not panic**: the generated-junk tests
  [`shell_hostile_input.rs`](../crates/openplugin-host/tests/shell_hostile_input.rs),
  [`clap_hostile_input.rs`](../crates/openplugin-targets/tests/clap_hostile_input.rs),
  [`lv2_hostile_input.rs`](../crates/openplugin-targets/tests/lv2_hostile_input.rs),
  [`lv2_atoms_hostile.rs`](../crates/openplugin-targets/tests/lv2_atoms_hostile.rs) and
  [`state_hostile_input.rs`](../crates/openplugin-core/tests/state_hostile_input.rs) (byte mutations
  of a state blob never panic and keep values valid).

How to write a plugin that does not panic: no `unwrap`/`expect`/indexing on data you did not size
yourself, `get`/`min` instead of `[..n]` with a host-supplied `n` (`StableFilter` clamps `frames` to
the buffer lengths; test `a_frame_count_larger_than_the_buffers_is_clamped_not_indexed`), checked
division, no integer overflow in release-profile arithmetic you do not control, and no
`RefCell`/`Mutex::lock().unwrap()` on the audio path.

### Findings (fixed)

- **The CLAP and LV2 entries now catch panics.** Every `extern "C"` entry in
  [`clap.rs`](../crates/openplugin-targets/src/clap.rs) and [`lv2.rs`](../crates/openplugin-targets/src/lv2.rs)
  runs under `catch_unwind`. A panic returns the failure value of the call (`CLAP_PROCESS_ERROR`,
  `false`, 0 or null). A panic in `process`/`run`, `params.flush`, `reset` or `activate` (CLAP), or in
  `run`, `connect_port`, `activate` or `deactivate` (LV2), poisons the instance: it writes silence
  to the outputs (and `CLAP_PROCESS_ERROR` for CLAP) and calls nothing in your plugin from
  `process` until the host activates it again. A panic in a state call fails that call and leaves
  the audio running. `catch_unwind` allocates nothing unless a panic happens; the no-allocation tests
  pass. The default panic hook still prints the message to stderr, on whichever thread panicked,
  including the audio thread; install your own hook with `std::panic::set_hook` in your plugin's
  init if that matters. Tests:
  [`clap_panic.rs`](../crates/openplugin-targets/tests/clap_panic.rs) (process, `set_param` from an
  event and from `params.flush`, note, reset, save, load),
  [`lv2_panic.rs`](../crates/openplugin-targets/tests/lv2_panic.rs) (run, `set_param` from a control
  port, note, deactivate, activate; the LV2 adapter publishes no state interface).
- **Containment through the shell is tested.** The shell now also zeroes the outputs when it catches a
  panic. [`shell_panic.rs`](../crates/openplugin-host/tests/shell_panic.rs): `process_events` and
  `process` return 0 with silent outputs, state calls return failure, and a later call works.
- **The floating-point mode comes back after a panic.** Every audio-thread entry (CLAP `process`,
  `params.flush` and `reset`, LV2 `run`, the shell's `process`, `process_events`, `reset` and
  `midi_out`) runs the plugin with flush-to-zero on and restores the host's own mode when the call
  ends; the guard is dropped on the panic path as well, so a panicking plugin cannot leave the host's
  audio thread flushing (or not flushing). See
  [PERFORMANCE.md](PERFORMANCE.md#3-denormals-and-flush-to-zero). Tests: the `a_panic_in_*` cases of
  [`clap_denormals.rs`](../crates/openplugin-targets/tests/clap_denormals.rs),
  [`lv2_denormals.rs`](../crates/openplugin-targets/tests/lv2_denormals.rs) and
  [`shell_denormals.rs`](../crates/openplugin-host/tests/shell_denormals.rs).
- **`Biquad::configure` rejects a NaN or infinite parameter** (the previous coefficients stay) and
  `Biquad` flushes its state below 1e-30. Tests in
  [`biquad.rs`](../crates/openplugin-dsp/src/biquad.rs).

Not covered: a panic in the plugin's constructor during CLAP `create_plugin` or LV2 `instantiate`
returns null by the same mechanism but has no test of its own.

## Thread rules

There are three kinds of thread, and the SDK's types say which one may touch what.

| Thread | May call | Must not |
| --- | --- | --- |
| **Audio** | `Plugin::process`, `process_buses`, `reset`, `set_param`/`set_param_index`, notes and MIDI, `set_transport`, `ParamCursor::pull`, `Seqlock::publish`, `Slot::poll` | allocate, lock, block, do I/O (section 1 of [PERFORMANCE.md](PERFORMANCE.md#1-the-real-time-rules)) |
| **Message / main** (state, editor, `prepare`) | `save_state`, `load_state`, `prepare`, every `Editor` call, `ParamPlane::store`, `Loader::submit` | be called from the audio thread: they allocate and may wait |
| **Worker** (`worker::Loader`) | the job closure you submit | touch the plugin object; it returns a value and the audio thread polls for it |

### State

`save_state` and `load_state` take `&mut self` of the plugin object, which is the same object
`process` runs on. One gate, `openplugin_core::PluginGate`
([`openplugin-core/src/gate.rs`](../crates/openplugin-core/src/gate.rs)), arbitrates in the shells and
in CLAP, and the audio thread never waits on it:

- `process` and `reset` take the gate with one compare-exchange. If a state call holds or has
  requested it, the block is **skipped**: input copied to output (dry), the plugin not entered,
  success returned. The skipped block's events are not lost: parameter events go to the parameter
  plane at once, and notes, MIDI and note expressions are delivered at the start of the next block,
  in their order (at most 1024 waiting in the shells, 256 in CLAP; more are dropped). A note-off in a
  skipped block therefore still ends its note.
- A state call serialises with other state calls on a mutex, raises a request bit, then waits for a
  block in flight to finish, at most 2 s in the shells (`STATE_WAIT`) and 250 ms in CLAP. On timeout
  it fails and changes nothing. New blocks see the request bit and skip, so the wait cannot be starved.
- The request bit and the in-flight bit are one atomic: for any pair of (block, state call) either
  the block sees the request and skips, or the state call sees the block and waits. They are never
  both inside the plugin.
- A plugin without a chunk takes no gate at all: its state is the parameter plane, which the audio
  thread only reads, and no block is ever skipped. That is a plugin that returns `true` from
  `state_is_parameters_only`, or whose `save_state` returns `None` when the instance is built
  (`openplugin_core::has_state_chunk`, asked once). So a plugin that ever saves a chunk must return
  `Some` from `save_state` on a new instance, before `prepare`; one that answers `None` then never has
  `save_state` or `load_state` called again.

What that means for you: you need **no lock** in `save_state` or `load_state` against `process`; they
do not overlap. Both may allocate. `load_state` must accept bytes from any version and any
corruption and return `false`, not panic.

Tests that enforce the gate:

| Rule | Test |
| --- | --- |
| A block that starts during a save is dry and does not wait | `a_block_that_starts_during_a_save_is_dry_and_does_not_wait` |
| The notes of a skipped block reach the next block, in order, once | `the_notes_of_a_block_skipped_by_a_save_reach_the_next_block` (also in `clap_state_concurrency.rs`) |
| A save waits for a block in flight | `a_save_waits_for_a_block_in_flight` |
| A state call gives up after the bounded wait and changes nothing | `a_call_gives_up_after_a_bounded_wait_and_changes_nothing` |
| Saves and loads beside `process` never tear | `saves_and_loads_beside_process_never_tear` |
| (all five above) | [`openplugin-host/tests/shell_state_threads.rs`](../crates/openplugin-host/tests/shell_state_threads.rs) |
| Plugins without a chunk, declared or not, never skip a block on save or load | `saves_and_loads_during_playback_never_skip_a_block` in [`state_param_only.rs`](../crates/openplugin-host/tests/state_param_only.rs) |
| A plugin with a chunk stays sound under state calls during playback | `chunk_plugin_state_calls_during_playback_stay_sound` (same file) |
| The audio thread does not allocate while a save or load runs | `the_audio_thread_does_not_allocate_during_a_save_or_a_load` in [`shell_state_no_alloc.rs`](../crates/openplugin-host/tests/shell_state_no_alloc.rs) |
| The same, through CLAP | `chunk_state_beside_process_is_never_torn`, `parameter_state_beside_process_is_never_torn_and_never_touches_the_plugin`, `parameter_load_is_applied_whole_or_not_at_all`, `audio_thread_does_not_allocate_during_saves_and_loads` in [`clap_state_concurrency.rs`](../crates/openplugin-targets/tests/clap_state_concurrency.rs) |
| A CLAP session of a plugin with a chunk holds the parameters beside it, and older CLAP sessions still load | `a_chunk_that_leaves_out_the_parameters_gets_them_back`, `sessions_saved_before_clap_wrote_sessions_still_load` (same file) |
| A state load applies whole or not at all, and corrupt state never touches the plugin | `restore_params_is_all_or_nothing`, `corrupt_session_envelopes_do_not_touch_the_plugin`, `plugin_load_state_rejects_random_and_corrupt_chunks` in [`state_hostile_input.rs`](../crates/openplugin-core/tests/state_hostile_input.rs) |

### Editor

Every `Editor` call is on the main thread. The editor never writes to the engine: it sends a gesture
through `EditorHost` in plain units, and the host returns the value to the engine as an ordinary
parameter change. Values that change elsewhere (automation, a preset, a state load) reach the editor
through `param_changed`. The editor reads meters from the audio thread through a `Seqlock`, which
never blocks the writer and never shows a torn frame. See [EDITOR.md](EDITOR.md#the-contract).
Tests: `readers_never_observe_a_torn_pair` in [`seqlock.rs`](../crates/openplugin-core/src/seqlock.rs);
the editor harness described in EDITOR.md (`cargo test -p example-editor`), which drives an editor
through the shell ABI, CLAP and LV2 on the main thread; and, for the audio side of the editor's data,
`concurrent_ordering` in [`spsc.rs`](../crates/openplugin-core/src/spsc.rs).

### Audio

The audio thread reads parameters from the plane (`ParamCursor::pull`), writes meters to a `Seqlock`
and receives built objects from `Slot::poll`; none of the three takes a lock or allocates. A heavy
object is built on the worker and swapped in: `worker_stress.rs` (20 000 swaps against a running
process loop) and `worker_no_alloc.rs` in [`openplugin-core/tests`](../crates/openplugin-core/tests),
and `convolve_ir_swap.rs` in [`openplugin-dsp/tests`](../crates/openplugin-dsp/tests/convolve_ir_swap.rs)
(a new impulse during playback).

## Host behaviours to survive

None of these is exotic, and each has crashed a shipped plugin. The third column says how the
conformance kit exercises it; the fourth where a test enforces it.

| What the host does | What your plugin must do | Kit | Test |
| --- | --- | --- | --- |
| **Calls `process` with a different frame count every time**: 1, 7, 333, 4096, then 2. Never more than the `max_block` it gave `prepare`, in most hosts; some exceed it | Produce correct output for any `frames <= max_block`; clamp, never index past a buffer; keep no state that assumes a fixed block | `Options::block_sizes` (default 1, 2, 3, 7, 16, 64, 333, 512, 1024, 4096, 8192) and a random-sequence plan inside every session | `block_sizes_that_change_on_every_call_are_fine`, `a_frame_count_larger_than_the_buffers_is_clamped_not_indexed` in [`stable_filter.rs`](../examples/perf_guide/tests/stable_filter.rs) |
| **Changes the sample rate: calls `prepare` again with a different rate and no `reset`** (a device switch, a project rate change) | Recompute everything that depends on the rate in `prepare`; keep working with the old state, which is a valid state of the same filter; never assume `prepare` runs once | `Options::sample_rates` (22.05 to 384 kHz) runs a *fresh* plugin per rate. A rate change on a live instance is **not** driven by the kit | `a_rate_change_without_a_reset_keeps_working` in [`stable_filter.rs`](../examples/perf_guide/tests/stable_filter.rs). Rates outside the supported range are refused and nothing is substituted: `sample_rate_range.rs` in [core](../crates/openplugin-core/tests/sample_rate_range.rs), [ffi](../crates/openplugin-host/tests/sample_rate_range.rs) and [targets](../crates/openplugin-targets/tests/sample_rate_range.rs) |
| **Calls `process` before `prepare` / before activate** (or after a failed `prepare`) | Not crash and not produce NaN. Either work with defaults, or do nothing | not driven | `process_before_prepare_is_defined` in [`stable_filter.rs`](../examples/perf_guide/tests/stable_filter.rs). In the SDK's FFI layer a block before `prepare` returns `false` (`process_block` and `process_stereo`: `audio.channels == 0`); there is no dedicated test |
| **Loads state during playback** | `load_state` returns `bool`; never leave half a state; do not assume the engine is stopped | state round-trip, truncated and random bytes into `load_state` (check `state`) | the gate tests in the thread section; `plugin_load_state_rejects_random_and_corrupt_chunks` |
| **Switches offline (freewheel) rendering on and off** | `set_offline` and `prepare` can arrive between any two blocks; don't allocate or change latency because of it | an `offline` scenario runs a session offline, `prepare`s again online, then offline again, with a random block plan, then a fixed 333, with no `reset` between | kit scenario in [`conformance.rs`](../crates/openplugin-testing/src/conformance.rs); `reprepare` also covers "prepare again with no reset" at one rate |
| **Sends NaN or infinity in the input** (a broken upstream plugin) | Do not keep it in state; recover | stimulus `nan_inf_bursts` (check `hostile_input`; `strict_hostile_output` to demand finite output during the burst) | `nan_and_infinity_at_the_input_do_not_reach_the_state`; `poisoned_by_nan_fails_hostile_input` in [`broken.rs`](../crates/openplugin-testing/tests/broken.rs) |
| **Writes parameters mid-block at sample offsets, out of range, NaN** | `set_param` clamps and returns `true` for a real id, `false` for an unknown one; `param` stays finite and in range | parameter fuzz (`Options::fuzz_sessions`), `param_extremes` (check `param_fuzz`) | `hostile_parameters_are_clamped` |
| **Calls `reset` in the middle of a tail** | Output silence afterwards | `reset` scenario (check `reset`) | the kit's own, proved on a deliberately wrong plugin in `broken.rs` |
| **Changes the layout** (mono, stereo, 5.1) while inactive | `set_layout` then `prepare_layout`; `process_buses` with the sidechain | `layouts` check over every `supported_layouts()` entry | the kit's own |
| **Reports a tail and then stops calling `process`** | Fall silent within the reported tail | `tail` check | `a_tail_decays_to_exact_zero_without_subnormals`; `tail_longer_than_reported_fails_tail` in `broken.rs` |

Two of these are not driven by the kit, and the table says so: a rate change on a live instance, and
`process` before `prepare`. Add them to your own tests; the two in `stable_filter.rs` are ten lines
each.

## Soak and fuzz testing with the kit

The kit is a fuzzer with a seed. It uses no clock and no OS randomness: the same seed on the same
plugin gives the same report, bit for bit, and every failure prints the seed, the case seed and the
case text that replays it (`Options::filter` takes any part of the case text).

```rust
// Every `cargo test`: the default seed.
conformance(MyPlugin::default, Options::default()).assert_pass();

// More fuzz per rate, different streams of parameters, noise and block sizes.
for seed in 1..=200u64 {
    let options = Options { fuzz_sessions: 12, ..Options::default() }.with_seed(seed);
    let report = conformance(MyPlugin::default, options);
    assert!(report.passed(), "seed {seed}: {report}");
}
```

- **A soak is many seeds.** `two_hundred_seeds_all_pass` in
  [`stable_filter.rs`](../examples/perf_guide/tests/stable_filter.rs) runs 200 seeds with 12 fuzz
  sessions per rate: 38.9 s in release on the machine of PERFORMANCE.md. It is `#[ignore]`d; run it
  with `cargo test --release -p example-perf-guide --test stable_filter -- --ignored`. Eight seeds run
  in every `cargo test` (`eight_seeds_with_extra_parameter_fuzz_all_pass`). Run the long one in CI
  nightly, not on every push.
- **Reproducing**: `OPENPLUGIN_TEST_SEED=0x00000000deadbeef cargo test` replays a seed from the
  environment; `Options::filter = Some(case_text)` replays one case. A failure report names both.
- **Sample rates and block sizes** are the options `sample_rates` and `block_sizes`; keep the
  defaults in release, use `Options::quick()` in debug builds (3 rates, 5 block sizes).
- **What to turn on for a plugin you want to be hard to break**: `strict_hostile_output`,
  `strict_denormals`, `verify_latency_by_impulse` where it is meaningful, and a `max_output` close to
  what the plugin can really produce (the default 64 is a ceiling for nonsense, not a spec).
- **The SDK's own fuzzers** use the same idea for the format entries: generated junk through
  `export_shell!`, CLAP and LV2 (`shell_hostile_input.rs`, `clap_hostile_input.rs`,
  `lv2_hostile_input.rs`, `lv2_atoms_hostile.rs`), through the state decoder
  (`state_hostile_input.rs`) and through the filters (`modulation_stress_is_bounded`).

## Case studies: bugs the kit found in the SDK's own code

The kit was run on the SDK's examples the day it was written, and found three real bugs. Each is
reproduced in this repository by a test that runs the kit on the *old* code and on the fix.

### `Biquad` kept a NaN for good (commit `ed32781`)

Symptom: the console EQ example failed `hostile_input`, and nothing else. After a burst of NaN and
infinity at the input, its output was NaN for every later block.

Cause: `Biquad::process_sample` wrote its input and output into the four state variables. A NaN in
the input became a NaN in `x1` and `y1`, then in every output, forever; only a host `reset` cleared it.

Fix, in the filter, not in the example (the example was right to trust the filter):

```rust
let x = if x.is_finite() { x } else { 0.0 };           // the input never reaches the state
let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
if !y.is_finite() { self.reset(); return 0.0; }          // and a bad result clears it
```

Proof it stays fixed: `example_console_eq_conforms` in
[`openplugin-testing/tests/examples.rs`](../crates/openplugin-testing/tests/examples.rs) (the test
was first written to assert that exactly `hostile_input` failed, and flipped to `assert_pass` with the
fix). Reproduction of the old filter and the new one through the kit:
`an_unguarded_biquad_keeps_a_nan_for_good_and_the_sdk_biquad_does_not` in
[`tests/kernels.rs`](../examples/perf_guide/tests/kernels.rs). The unguarded filter also fails
`no_denormals`: its tail decays through the subnormal range, which is still true of `Biquad` today
(table above: it does not flush).

### The tone filter of `design_editor` kept a NaN and decayed through denormals (commit `e34f820`)

Symptom (reproduced; the kit's report for the old code):

```text
  [FAIL] no_denormals    no denormal numbers in the output (78106 assertions, 58 failed)
         first failure: output is denormal (9.952187e-39) at channel 0, sample 321 of a 512-frame block [after 8704 frames, block 17]
         case: reset rate=48000
  [FAIL] hostile_input   recovers from NaN and infinity at the input (4823 assertions, 4823 failed)
         first failure: output is NaN at channel 0, sample 0 of a 512-frame block [after 56832 frames, block 111]
         case: rates rate=44100 max_block=4096 layout=default offline=false stimulus=nan_inf_bursts plan=fixed512
         reproduce: seed 0x000000000be05eed (case seed 0x97a815f0c05d2e2a)
```

Cause: `low += a * (*s * gain - low)`, a one-pole low-pass, with its state `low` kept across blocks.
Two bugs in one line of state: a NaN stays in `low` for good; and a decaying `low` passes through
`1e-38` on its way to zero.

Fix, one condition after the update:

```rust
if !low.is_finite() || low.abs() < 1e-20 { low = 0.0; }
```

Proof: [`example_design_editor_conforms`](../crates/openplugin-testing/tests/examples.rs) runs the
example through the kit; the old code is reproduced by `ToneFilter { guard: false }` in
[`kernels.rs`](../examples/perf_guide/src/kernels.rs) and
`the_unguarded_tone_filter_fails_the_kit_where_it_failed_in_the_example` in
[`tests/kernels.rs`](../examples/perf_guide/tests/kernels.rs), which asserts both checks fail without
the guard and the kit passes with it.

### What the kit has not found yet

`gain` and `editor` passed on the first run. The kit's `locks` check is permanently `skip`; a lock
on the audio path is invisible to it and to every test above, which is why PERFORMANCE.md names the
thread sanitizer as the tool for that rule.

## Checklist

1. `conformance(...)` passes, in `--release` with `Options::default()` and in debug with `Options::quick()`.
2. A soak over many seeds runs on a schedule.
3. Anything modulated or resonant is a state variable filter; recursive state is `f64`.
4. Every parameter is clamped on the way in (`ParamInfo::clamp`, which also rejects NaN) and smoothed on the way to a coefficient.
5. Non-finite input never reaches state; an out-of-range result clears it; decaying state is flushed.
6. `process` can run before `prepare`, with any `frames` up to the buffer length, after a `prepare` at a new rate with no `reset`.
7. `load_state` returns `false` on anything it does not understand and never panics.
8. Nothing on the audio path can panic: no `unwrap`, no unchecked index, no `Mutex::lock().unwrap()`.
9. Editor and state code run on their own threads and talk to the audio thread only through the plane, the `Seqlock`, the SPSC queue or the worker.
