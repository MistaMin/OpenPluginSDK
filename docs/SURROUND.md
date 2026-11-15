# Stereo plugins on surround buses

`openplugin_dsp::SurroundAdapter` lets a plugin written for stereo run on 5.0, 5.1, 7.1, 7.1.4, 5.1.4 and other surround buses. It is itself a `Plugin`, so the export changes by one line:

```rust
// before
export_clap!(MyCompressor);
// after (default options)
export_clap!(StereoOnSurround<MyCompressor>);
```

`StereoOnSurround<P>` is `SurroundAdapter<P, fn() -> P>` with a `Default` impl, which is what the export macros need. The options below need a builder call, so to export an adapter with options, wrap it in a small type of your own that implements `Default` (and forwards `Plugin`), or call the builder where you construct the plugin yourself.

The adapter lives in `openplugin-dsp`, not `openplugin-core`: its fold-to-stereo mode uses `Downmix`, and core cannot depend on dsp. It uses only the public `Plugin` trait and the channel-role helpers of `openplugin_core::layout`.

It is opt-in. A widener, a reverb or anything that needs speaker positions should have its own surround code: the adapter treats every pair as an independent stereo signal.

## Options

```rust
SurroundAdapter::new(MyCompressor::default)
    .surround_layouts(&[SpeakerLayout::Surround51, SpeakerLayout::Surround714])
    .fold_layouts(&[SpeakerLayout::Surround51])   // surround in, stereo out
    .lfe(LfeMode::Pass)                           // Process (default), Pass or Mute
    .fold_settings(MixSettings::default())        // BS.775 levels
    .linked()                                     // needs LinkedDetector
    .link_interval(32)                            // frames between detector exchanges
```

`new` publishes the inner plugin's own layouts (the first stays the default), then 5.0, 5.1, 7.1, 7.1.4 and 5.1.4 with matching input and output, then the fold layouts (none by default). Surround and fold layouts have no sidechain. A layout the inner plugin lists itself goes straight to it, sidechain included. If the inner plugin does not list stereo, only its own layouts are published.

## Engines

On a surround layout the adapter runs, in this order:

1. one engine per left/right pair from `SpeakerLayout::pairs()` (front, sides, backs, front wides, height pairs, top sides), prepared as stereo;
2. one engine per unpaired non-LFE channel (centre, back centre, top centre). If the inner plugin lists mono in and out, it is prepared as mono. Otherwise it is prepared as stereo, fed the channel on both inputs, and the **left** output is taken. A stereo-linked, symmetric plugin sounds the same either way;
3. for `LfeMode::Process`, one more such engine for the LFE. `Pass` leaves the LFE as it is; `Mute` writes zeros.

Engines are created from the factory in `prepare_layout` (allocation allowed there) and never on the audio path. A new engine starts with the first engine's parameters and state.

## Parameters, state and reads

- `set_param` and `set_param_index` go to every engine. `param`, `meters`, `bus_meters`, `latency`, `tail` and `save_state` come from the first engine, which is the front pair on a surround layout. `load_state` loads into all.
- `gain_reduction_db` is the largest of any engine.
- All engines have the same latency, so the bus latency is the inner plugin's. The tail is the inner tail.
- MIDI, transport, offline and bypass go to every engine.

## Shared detector

A compressor on five pairs with independent detectors pulls the image around: the loud pair is turned down and the quiet pairs are not. Implement `LinkedDetector` on the inner plugin and call `.linked()`:

```rust
impl LinkedDetector for MyCompressor {
    fn detector_level(&self) -> f32 { self.envelope }       // >= 0, bigger asks for more change
    fn set_linked_level(&mut self, level: f32) { self.linked = level; }
}
// in process: let effective = self.envelope.max(self.linked);
```

The unit of the level is the plugin's choice; the adapter only takes the maximum. The adapter processes in chunks of `link_interval` frames (default 32). After each chunk it takes the largest `detector_level` of the linked engines and calls `set_linked_level` on all of them. The LFE engine is not part of the link, so a loud bass does not clamp the program. It resets the link to 0.0 on `reset` and `prepare_layout`; a plugin that does not implement the trait runs independently.

Limits: the link lags by one chunk, so a transient on one pair reaches the others up to `link_interval` frames late. It is exact for steady and slowly moving signals. Chunks restart at each host block, so a linked output depends slightly on the block boundaries. An unlinked adapter is block-size independent whenever the inner plugin is.

## Fold to stereo

A fold layout is surround in, stereo out (`IoConfig { input: Surround51, output: Stereo, .. }`). Per block the adapter computes `stereo = Downmix(input)` (BS.775 by default; `fold_settings` changes the levels, and the LFE is dropped unless `lfe_db` is set), runs one inner engine on `stereo`, and writes the result to channels 0 and 1. The other channels are zeroed. So `out = inner(Downmix(in))`, with no added latency. The roles come from the layout, so every SDK layout with a speaker map folds.

## Proof

`crates/openplugin-dsp/tests/surround_adapter.rs`, with a gain plus compressor test plugin:

- 5.0, 5.1, 7.1.4 and 5.1.4 in every LFE mode are bit-identical to engines run by hand on the pairs (odd block sizes);
- the mono path and the duplicate path agree;
- with `linked()`, bit-identical to a by-hand run of the same chunk exchange, and the gain on every non-LFE channel is equal (to 1e-5) while the independent run leaves the quiet channels alone;
- fold output is bit-identical to the inner plugin run on the `Downmix` of the input; the other channels are silent;
- parameters, state, latency and tail; layouts are pooled and refuse what they do not list;
- the conformance kit passes (linked, with fold layouts), and `process` does not allocate in any mode.

Not covered yet: the VST3, Audio Unit and AAX shell tables for the surround layouts (roadmap 0.6 task 8 follow-ups), so "in every format" waits on those.
