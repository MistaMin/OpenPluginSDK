# Per-note expression

Roadmap 0.7 task 4. A host can bend, press, brighten, pan or swell one note of a chord, and modulate a parameter for one note only. The SDK carries that from every format to the voices in `openplugin-dsp`:

| Format | What arrives | How |
|---|---|---|
| CLAP | note ids on note-on and note-off, `CLAP_EVENT_NOTE_EXPRESSION` (all seven ids), `CLAP_EVENT_PARAM_MOD`; out: `voice-info` and `CLAP_EVENT_NOTE_END` | Rust entry, `crates/openplugin-targets/src/clap.rs` |
| CLAP (MIDI dialect), LV2 | MPE over MIDI | the shared `MpeReceiver` |
| VST3 | note ids, `kNoteExpressionValueEvent`; `INoteExpressionController` and `INoteExpressionPhysicalUIMapping` on the controller | shell ABI 7 ([SHELL.md](SHELL.md#per-note-expression-abi-7)) |
| Audio Unit, AAX | MPE over MIDI; the Audio Unit reports `kAudioUnitProperty_SupportsMPE` | the shells pass MIDI as before; the library runs the `MpeReceiver` |

Not checked in a DAW yet: the proofs below are fake hosts.

## In a plugin

Opt in; a plugin that does not sees exactly what it saw before.

```rust,ignore
use openplugin::core::{Expression, ExpressionSet, MpeZones, NoteRef, ParamFlags, ParamInfo, Plugin, Unit, VoiceInfo};
use openplugin::dsp::VoiceAllocator;

static PARAMS: [ParamInfo; 1] = [
    // Per-note (polyphonic) modulation: CLAP_PARAM_IS_MODULATABLE and the four PER_* flags.
    ParamInfo::new("tone", "Tone", 0.0, 1.0, 0.0, Unit::None)
        .with_flags(ParamFlags::AUTOMATABLE.union(ParamFlags::POLY_MODULATION)),
];

impl Plugin for Synth {
    // ...
    fn note_expressions(&self) -> ExpressionSet { ExpressionSet::ALL }
    fn mpe_input(&self) -> Option<MpeZones> { Some(MpeZones::LOWER_15) }

    fn note_on_ref(&mut self, _offset: usize, note: NoteRef, velocity: u8) {
        self.voices.note_on_ref(note, velocity);
    }
    fn note_off_ref(&mut self, _offset: usize, note: NoteRef, _velocity: u8) {
        while self.voices.note_off_ref(note).released.is_some() {}
    }
    fn note_expression(&mut self, _offset: usize, note: NoteRef, expression: Expression, value: f32) {
        self.voices.note_expression(note, expression, value);
    }
    fn param_modulation(&mut self, _offset: usize, index: usize, note: NoteRef, amount: f32) {
        if index == TONE { self.voices.poly_modulation(note, 0, amount); }
    }
    // CLAP voice-info, and CLAP_EVENT_NOTE_END for each voice that has ended.
    fn voice_info(&self) -> Option<VoiceInfo> { Some(self.voices.voice_info()) }
    fn pull_note_end(&mut self) -> Option<NoteRef> { self.voices.pop_ended() }
}
```

Each voice then reads `voice.pitch()` (its tuning included), `voice.expression(Expression::Pressure)` and so on, and `voice.modulation(0)`. `examples/expression_synth` is a complete eight-voice synth that does this.

- `NoteRef { id, port, channel, key }`: `-1` is "any" (the CLAP wildcard rule). A note-on carries the host's note id, or `-1` when the host has none.
- **Routing** (`NoteRef::matches`): when the event and the voice both have a note id, the ids decide. Otherwise port, channel and key decide, `-1` matching anything; an id-only event never reaches a voice started without an id. A host with no note ids (MIDI, MPE, the Audio Unit, AAX, LV2) is matched by channel and key, and MPE gives each note its own channel, so two notes of the same key on two channels are two voices.
- **Values** are in CLAP's units (`Expression::range`): volume a linear gain `0..=4` (1 is unity), pan `0` left to `1` right, tuning `-120..=120` semitones, the rest `0..=1`. Every entry clamps and drops NaN and infinities before the plugin sees them. A value is a statement, not a step: it replaces the last one.
- **Smoothing**: `NoteExpressions` (one per voice in `VoiceAllocator`, usable on its own) ramps each value linearly over `set_expression_smoothing` (5 ms by default) as `Voice::advance` runs, like the glide. A value sent before the voice has played a sample (at the note-on's own sample, or MPE's initial state) is taken at once, so the note starts there.
- **Modulation**: `ParamFlags::MODULATABLE` is whole-plugin modulation only (`note` is `NoteRef::ALL`); `ParamFlags::POLY_MODULATION` adds per note id, key, channel and port. The amount is in the parameter's plain units, clamped to its span, and never moves or saves the parameter's value.

## Voices and note ends

A host that models the plugin's voices (to send polyphonic modulation, say) needs to know how many there are and when each one stops sounding:

- `Plugin::voice_info` (default `None`) returns a `VoiceInfo { count, capacity, overlapping_notes }`. The CLAP entry offers `voice-info` only when it is `Some`, and reads it each time the host calls `get` (main thread), so a polyphony setting shows at the host's next look. `VoiceAllocator::voice_info` gives the voices new notes may use (`set_max_voices`; 1 in mono and legato) and `N`; notes do not overlap, since a key held again on the same channel retriggers its voice.
- `Plugin::pull_note_end` (default `None`) hands over one ended note at a time. After every block the CLAP entry drains it and sends each as `CLAP_EVENT_NOTE_END` with the note-on's note id, port, channel and key, at the block's last sample and after the block's MIDI output, so the events stay in time order. With no output list, or a full one, the ends of that block are dropped rather than sent late. Nothing allocates.
- `VoiceAllocator` keeps the list (`pop_ended`, at most `ENDED_CAPACITY` = 64 waiting): a note ends when its voice is freed after its release, or when the voice is taken for a different note (stolen, a same-note tail reused, a mono or legato change, a held key retriggered with a new note id). A key retriggered with the same note does not end. `reset` clears the list along with the voices, since a host that resets the plugin forgets its notes itself. A note from MIDI or MPE ends with id `-1` and its channel and key.
- The Audio Unit shell reports `kAudioUnitProperty_SupportsMPE` (global, read-only `UInt32`): 1 for a plugin whose `mpe_input` is `Some` and that has a note input, else 0. The library says so with the `OP_NOTE_PORT_MPE` bit of `op_shell_note_ports` ([SHELL.md](SHELL.md#per-note-expression-abi-7)).

## MPE

`openplugin_core::mpe::MpeReceiver`, written from the MMA/AMEI RP-053 *MIDI Polyphonic Expression* 1.0 (2018; 1.1, M1-100-UM, changes wording only). Every MIDI-carrying entry feeds it, for a plugin whose `mpe_input` is `Some`:

- **Zones** (2.1.1): the MPE Configuration Message (RPN 6) on channel 1 sets the Lower Zone (members from channel 2 up), on channel 16 the Upper Zone (from 15 down); 0 members turns a zone off; the newest message wins where zones would overlap. Channels entering or leaving a zone have their notes stopped and controls reset (2.1.4).
- **Pitch bend** (2.4): ±48 semitones on members and ±2 on the master after an MCM; RPN 0 on the master sets the master range, on any member every member's range. A note's tuning is its channel's bend plus the master's.
- **Pressure and timbre** (2.5, 2.6): channel pressure and CC 74, each `value / 127`; the master's value adds its offset from rest (pressure 0, CC 74 64) to the member's.
- **Initial state** (3.3): bend, pressure and CC 74 sent before a note-on start the note there. Member messages stop at note-off; the master's bend reaches released notes too (3.3.3). Note-on with velocity 0 is a note-off with velocity 64 (3.3.2).
- **Zone messages** (2.3.1): other controllers and program change on a member channel are ignored; on the master they pass through as raw MIDI.
- **Channels in no zone** are plain MIDI.

With VST3, a note that carries a note id is never fed to MPE: the host addresses it with its own note expressions.

## Proof

- `examples/expression_synth/tests/formats.rs`: CLAP note expressions, CLAP polyphonic modulation (and its flags), MPE over CLAP's MIDI dialect, MPE through the LV2 entry, the shell ABI with note ids and expressions and with MPE over plain MIDI (the Audio Unit and AAX path), the ABI 7 description calls; each with a negative control and hostile input; no allocation in any of those audio paths (counting allocator); the conformance kit.
- `native/vst3/expression_host.cpp` (`vst3_expression_proof`): the VST3 shell over Expression Synth. Reads `INoteExpressionController` (seven types, titles, units, defaults, bipolar flags, text both ways) and the physical UI mapping, plays notes with ids 1 and 2, and measures with the Goertzel algorithm that pressure, brightness and tuning sent for id 1 change only that voice's partials; id 99 changes nothing; no `operator new` in `process`.
- Voices and note ends: in `formats.rs`, CLAP `voice-info` reads 8 of 8; two notes with ids, ports and channels, one released: exactly one `NOTE_END` with that note's id, port, channel and key, and none for the held one (the negative control) until it is released; a stolen voice and an MPE note each end once; ends from blocks with no output list are not sent later; the NOTE_END path is inside the no-allocation measurement. `au_mpe_proof` and `au_mpe_proof_off` (`native/au/mpe_host.cpp`) read `SupportsMPE` from the shell over Expression Synth (1) and over the example (0), before and after `Initialize`, and check that a host cannot set it. clap-validator passes on both examples (CI's `clap-validator` job).
- Unit tests: `crates/openplugin-core/src/mpe.rs` (zones, both zones, pitch bend range changes, master messages, a channel with no zone, hostile bytes), `expression.rs`, `crates/openplugin-dsp/src/voice.rs` and `voice_alloc.rs`.

## Not done

- VST3 has no polyphonic parameter modulation in its API; `NoteOnEvent::tuning` (cents at note-on) is not carried.
- AAX does not declare MPE support to the host (it has no such property); MPE reaches the plugin when the host sends it.
- `CLAP_EVENT_NOTE_END` carries the block's last sample, not the sample the voice ended on. `voice-info` changes are not announced (`clap_host_voice_info::changed`); the host sees them when it next asks.
