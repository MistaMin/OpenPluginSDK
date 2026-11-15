//! A block with events, transport, a sidechain and MIDI output: the engine
//! side of `op_shell_process_events`.
//!
//! Events are copied into a list allocated at creation, stably sorted by
//! offset, and delivered the way the CLAP bridge does: the block is cut at
//! each distinct offset, the events at that offset are applied (notes and raw
//! MIDI to the plugin at sample 0 of the next span, parameters to the plane),
//! and the following span is processed. Nothing here allocates or panics on
//! the audio path; a plugin panic is caught by the caller.
//!
//! Channels cross this boundary in the SDK's canonical order. The shells
//! reorder from the host's order at their own edge.

use std::sync::atomic::Ordering;

use openplugin_core::{mpe, Expression, IoConfig, NoteRef, Transport, MAX_AUDIO_CHANNELS};

use crate::{Audio, FfiInstance};

/// Most events one block may carry. A longer list is refused whole.
pub const MAX_EVENTS: usize = 1024;
/// MIDI output events kept per block. Later ones are dropped and counted.
pub const MAX_MIDI_OUT: usize = 1024;

pub const EVENT_NOTE_ON: u32 = 1;
pub const EVENT_NOTE_OFF: u32 = 2;
pub const EVENT_MIDI: u32 = 3;
pub const EVENT_PARAM_PLAIN: u32 = 4;
pub const EVENT_PARAM_NORMALIZED: u32 = 5;
// ---- ABI 7: per-note expression ----
/// A note-on with the host's note id in `index` (the bits of an `i32`; -1,
/// `0xFFFF_FFFF`, for none), `channel`, `note` and `velocity` as
/// [`EVENT_NOTE_ON`]. Delivered to `Plugin::note_on_ref`; a note without an id
/// goes through MPE for a plugin that asked for it, one with an id never does.
pub const EVENT_NOTE_ON_ID: u32 = 6;
/// A note-off with the host's note id in `index`, as [`EVENT_NOTE_ON_ID`];
/// `velocity` is the release velocity.
pub const EVENT_NOTE_OFF_ID: u32 = 7;
/// A per-note expression: `index` the note id (or -1), `midi[0]` the
/// expression (`OP_EXPRESSION_*`, the CLAP ids), `channel` and `note` the
/// channel and key ([`NOTE_ANY`] for any), `value` in the expression's units
/// ([`Expression::range`]). Delivered to `Plugin::note_expression` when the
/// plugin lists the expression, clamped; NaN is skipped.
pub const EVENT_NOTE_EXPRESSION: u32 = 8;
/// `channel` or `note` of an [`EVENT_NOTE_EXPRESSION`]: any.
pub const NOTE_ANY: u8 = 0xFF;

/// One sample-offset event. Fields not used by `kind` are ignored.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OpEvent {
    /// Sample offset inside the block, below `frames`.
    pub offset: u32,
    pub kind: u32,
    /// Parameter index for the two parameter kinds.
    pub index: u32,
    /// Plain or normalized value for the parameter kinds.
    pub value: f32,
    /// MIDI channel 0..15 for note events.
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    pub reserved: u8,
    /// Status and two data bytes for `EVENT_MIDI`; the fourth byte is unused.
    pub midi: [u8; 4],
}

/// One MIDI output event: block offset and three bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpMidiEvent {
    pub offset: u32,
    pub data: [u8; 4],
}

pub const TRANSPORT_PLAYING: u32 = 1;
pub const TRANSPORT_RECORDING: u32 = 2;
pub const TRANSPORT_HAS_TEMPO: u32 = 4;
pub const TRANSPORT_HAS_SIGNATURE: u32 = 8;
pub const TRANSPORT_HAS_BEATS: u32 = 16;
pub const TRANSPORT_HAS_SECONDS: u32 = 32;

/// Host transport. A field whose `HAS_` flag is clear is taken as the
/// [`Transport`] default.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OpTransport {
    pub flags: u32,
    pub numerator: u32,
    pub denominator: u32,
    pub reserved: u32,
    pub tempo: f64,
    pub sample_pos: i64,
    pub seconds: f64,
    pub quarter_notes: f64,
    pub bar_start: f64,
    pub bar_number: i64,
}

impl OpTransport {
    pub fn to_transport(&self) -> Transport {
        let mut out = Transport {
            playing: self.flags & TRANSPORT_PLAYING != 0,
            recording: self.flags & TRANSPORT_RECORDING != 0,
            sample_pos: self.sample_pos.max(0),
            ..Transport::default()
        };
        if self.flags & TRANSPORT_HAS_TEMPO != 0 && self.tempo.is_finite() {
            out.tempo = self.tempo;
        }
        if self.flags & TRANSPORT_HAS_SIGNATURE != 0 {
            out.numerator = self.numerator;
            out.denominator = self.denominator;
        }
        if self.flags & TRANSPORT_HAS_BEATS != 0 {
            if self.quarter_notes.is_finite() {
                out.quarter_notes = self.quarter_notes;
            }
            if self.bar_start.is_finite() {
                out.bar_start = self.bar_start;
            }
            out.bar_number = self.bar_number;
        }
        if self.flags & TRANSPORT_HAS_SECONDS != 0 && self.seconds.is_finite() {
            out.seconds = self.seconds;
        }
        out
    }
}

/// Everything one block needs. `struct_size` is `sizeof(OpProcess)` as the
/// caller compiled it; a smaller value is refused, so the struct can grow at
/// the end.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpProcess {
    pub struct_size: u32,
    pub frames: u32,
    /// `input_channels` pointers to `frames` floats; may be null when the
    /// count is 0 (an instrument). A null channel pointer reads as silence.
    pub inputs: *const *const f32,
    pub input_channels: u32,
    pub output_channels: u32,
    pub outputs: *const *mut f32,
    /// Optional sidechain, `sidechain_channels` pointers; null and 0 for none.
    pub sidechain: *const *const f32,
    pub sidechain_channels: u32,
    pub event_count: u32,
    pub events: *const OpEvent,
    /// Null leaves the plugin's transport as it was.
    pub transport: *const OpTransport,
}

/// The engine channel an output channel reads from.
fn engine_channel(index: usize, channels: usize) -> usize {
    index.min(channels.saturating_sub(1))
}

/// Input pointer feeding engine channel `index`: its own channel, or channel
/// 0 when the caller has exactly one input (mono into stereo), else none.
///
/// # Safety
/// `inputs` addresses `count` pointers.
unsafe fn input_for(inputs: *const *const f32, count: usize, index: usize) -> *const f32 {
    if inputs.is_null() || count == 0 {
        return std::ptr::null();
    }
    if index < count {
        unsafe { *inputs.add(index) }
    } else if count == 1 {
        unsafe { *inputs }
    } else {
        std::ptr::null()
    }
}

/// # Safety
/// `src` is null or addresses `start + dst.len()` floats.
unsafe fn read_span(src: *const f32, start: usize, dst: &mut [f32]) {
    if src.is_null() {
        dst.fill(0.0);
        return;
    }
    for (i, out) in dst.iter_mut().enumerate() {
        let sample = unsafe { *src.add(start + i) };
        *out = if sample.is_finite() { sample } else { 0.0 };
    }
}

/// The note id an ABI 7 event carries in `index`; `None` below -1.
fn event_note_id(event: &OpEvent) -> Option<i32> {
    let id = event.index as i32;
    (id >= -1).then_some(id)
}

fn valid_event(event: &OpEvent, frames: usize) -> bool {
    if event.offset as usize >= frames {
        return false;
    }
    match event.kind {
        EVENT_NOTE_ON | EVENT_NOTE_OFF => {
            event.channel < 16 && event.note < 128 && event.velocity < 128
        }
        EVENT_NOTE_ON_ID | EVENT_NOTE_OFF_ID => {
            event.channel < 16 && event.note < 128 && event.velocity < 128 && event_note_id(event).is_some()
        }
        EVENT_NOTE_EXPRESSION => {
            event_note_id(event).is_some()
                && Expression::from_id(i64::from(event.midi[0])).is_some()
                && (event.channel < 16 || event.channel == NOTE_ANY)
                && (event.note < 128 || event.note == NOTE_ANY)
                && event.value.is_finite()
        }
        EVENT_MIDI => {
            event.midi[0] & 0x80 != 0 && event.midi[1] < 0x80 && event.midi[2] < 0x80
        }
        EVENT_PARAM_PLAIN | EVENT_PARAM_NORMALIZED => event.value.is_finite(),
        _ => false,
    }
}

impl FfiInstance {
    fn apply_events(&self, audio: &mut Audio, start: usize, end: usize) {
        for index in start..end {
            let event = audio.events[index];
            if let Some(receiver) = audio.mpe.as_mut() {
                // MPE: notes without a host note id and raw MIDI go through
                // the receiver. A note with an id (VST3) does not: the host
                // addresses it with its own per-note expressions.
                let no_id = event_note_id(&event).is_none_or(|id| id < 0);
                let bytes = match event.kind {
                    EVENT_NOTE_ON => Some([0x90 | event.channel, event.note, event.velocity]),
                    EVENT_NOTE_OFF => Some([0x80 | event.channel, event.note, event.velocity]),
                    EVENT_NOTE_ON_ID if no_id => Some([0x90 | event.channel, event.note, event.velocity]),
                    EVENT_NOTE_OFF_ID if no_id => Some([0x80 | event.channel, event.note, event.velocity]),
                    EVENT_MIDI => Some([event.midi[0], event.midi[1], event.midi[2]]),
                    _ => None,
                };
                if let Some(bytes) = bytes {
                    mpe::receive_into(receiver, audio.inner.as_mut(), 0, bytes);
                    continue;
                }
            }
            match event.kind {
                EVENT_NOTE_ON => {
                    audio
                        .inner
                        .note_on_channel(0, event.channel, event.note, event.velocity)
                }
                EVENT_NOTE_OFF => audio.inner.note_off(0, event.note),
                EVENT_NOTE_ON_ID | EVENT_NOTE_OFF_ID => {
                    let id = event_note_id(&event).unwrap_or(-1);
                    let note = NoteRef::new(id, 0, i16::from(event.channel), i16::from(event.note));
                    if event.kind == EVENT_NOTE_ON_ID {
                        audio.inner.note_on_ref(0, note, event.velocity);
                    } else {
                        audio.inner.note_off_ref(0, note, event.velocity);
                    }
                }
                EVENT_NOTE_EXPRESSION => {
                    let Some(expression) = Expression::from_id(i64::from(event.midi[0])) else { continue };
                    if !audio.expressions.contains(expression) {
                        continue;
                    }
                    let Some(value) = expression.sanitize(f64::from(event.value)) else { continue };
                    let any = |v: u8| if v == NOTE_ANY { -1 } else { i16::from(v) };
                    let id = event_note_id(&event).unwrap_or(-1);
                    let note = NoteRef::new(id, -1, any(event.channel), any(event.note));
                    audio.inner.note_expression(0, note, expression, value);
                }
                EVENT_MIDI => match event.midi[0] & 0xF0 {
                    0x90 => audio.inner.note_on_channel(
                        0,
                        event.midi[0] & 0x0F,
                        event.midi[1],
                        event.midi[2],
                    ),
                    0x80 => audio.inner.note_off(0, event.midi[1]),
                    _ => audio
                        .inner
                        .midi(0, [event.midi[0], event.midi[1], event.midi[2]]),
                },
                EVENT_PARAM_PLAIN => {
                    self.plane.store(event.index as usize, event.value);
                }
                EVENT_PARAM_NORMALIZED => {
                    self.store_normalized(event.index as usize, event.value);
                }
                _ => {}
            }
        }
    }

    /// Process `[start, end)` of the block from the caller's buffers.
    ///
    /// # Safety
    /// Pointers in `args` are valid for `frames` floats per channel.
    unsafe fn run_span(&self, audio: &mut Audio, args: &OpProcess, start: usize, end: usize, side: bool) {
        let n = end - start;
        if n == 0 {
            return;
        }
        let channels = audio.channels;
        for (c, buffer) in audio.buffers.iter_mut().enumerate().take(channels) {
            let src = unsafe { input_for(args.inputs, args.input_channels as usize, c) };
            unsafe { read_span(src, start, &mut buffer[..n]) };
        }
        for (c, channel) in audio.sidechain.iter_mut().enumerate() {
            let src = if side && c < args.sidechain_channels as usize {
                unsafe { *args.sidechain.add(c) }
            } else {
                std::ptr::null()
            };
            unsafe { read_span(src, start, &mut channel[..n]) };
        }
        audio.cursor.pull(&self.plane, audio.inner.as_mut());
        {
            let Audio {
                inner,
                buffers,
                sidechain,
                ..
            } = &mut *audio;
            let side_arg = if side && !sidechain.is_empty() {
                Some(sidechain.as_slice())
            } else {
                None
            };
            inner.process_buses(&mut buffers[..channels], side_arg, n);
        }
        // Drain the plugin's MIDI output; bounded so a plugin that never stops cannot hang the block.
        let mut budget = MAX_MIDI_OUT * 2;
        while budget > 0 {
            budget -= 1;
            let Some((time, data)) = audio.inner.pull_midi() else { break };
            if audio.midi_out.len() < MAX_MIDI_OUT {
                let offset = (start as u32).saturating_add(time.min(n.saturating_sub(1) as u32));
                audio.midi_out.push(OpMidiEvent {
                    offset,
                    data: [data[0], data[1], data[2], 0],
                });
            } else {
                self.midi_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        for c in 0..args.output_channels as usize {
            let dst = unsafe { *args.outputs.add(c) };
            if dst.is_null() {
                continue;
            }
            let src = &audio.buffers[engine_channel(c, channels)][..n];
            for (i, sample) in src.iter().enumerate() {
                unsafe { *dst.add(start + i) = *sample };
            }
        }
    }

    /// Dry block: the input (or silence) goes to the output.
    ///
    /// # Safety
    /// Pointers in `args` are valid for `frames` floats per channel.
    unsafe fn dry(args: &OpProcess) {
        let frames = args.frames as usize;
        for c in 0..args.output_channels as usize {
            let dst = unsafe { *args.outputs.add(c) };
            if dst.is_null() {
                continue;
            }
            let src = unsafe { input_for(args.inputs, args.input_channels as usize, c) };
            for i in 0..frames {
                let sample = if src.is_null() { 0.0 } else { unsafe { *src.add(i) } };
                unsafe { *dst.add(i) = if sample.is_finite() { sample } else { 0.0 } };
            }
        }
    }

    /// One block with events, transport and sidechain.
    ///
    /// Returns false, touching no audio and no plugin state, for a struct
    /// that is too small, a null array with a non-zero count, more than
    /// [`MAX_EVENTS`] events, more than [`MAX_AUDIO_CHANNELS`] channels in
    /// any bus, or a block longer than the prepared size. An individual event
    /// with an offset at or past `frames`, an unknown kind or an out-of-range
    /// value is ignored. A state call holding the gate makes the block dry
    /// (events lost) and still returns true.
    ///
    /// # Safety
    /// Every pointer in `args` is null (where allowed) or valid for the count
    /// and `frames` it states; an input may alias an output.
    pub unsafe fn process_block(&self, args: &OpProcess) -> bool {
        if (args.struct_size as usize) < std::mem::size_of::<OpProcess>() {
            return false;
        }
        let max = MAX_AUDIO_CHANNELS as u32;
        if args.input_channels > max || args.output_channels > max || args.sidechain_channels > max {
            return false;
        }
        if (args.input_channels > 0 && args.inputs.is_null())
            || (args.output_channels > 0 && args.outputs.is_null())
            || (args.sidechain_channels > 0 && args.sidechain.is_null())
            || (args.event_count > 0 && args.events.is_null())
            || args.event_count as usize > MAX_EVENTS
        {
            return false;
        }
        let frames = args.frames as usize;
        if frames == 0 {
            return true;
        }
        let Some(_gate) = self.enter_audio() else {
            unsafe { Self::dry(args) };
            return true;
        };
        let audio = unsafe { &mut *self.audio.get() };
        if frames > audio.max_block || audio.channels == 0 {
            return false;
        }
        if !args.transport.is_null() {
            let transport = unsafe { &*args.transport }.to_transport();
            audio.inner.set_transport(&transport);
        }
        self.apply_offline(audio);
        audio.midi_out.clear();
        audio.midi_read = 0;

        // Valid events, stably sorted by offset, in the list made at creation.
        audio.events.clear();
        for i in 0..args.event_count as usize {
            let event = unsafe { *args.events.add(i) };
            if !valid_event(&event, frames) {
                continue;
            }
            let mut at = audio.events.len();
            while at > 0 && audio.events[at - 1].offset > event.offset {
                at -= 1;
            }
            audio.events.insert(at, event);
        }

        let side = args.sidechain_channels > 0
            && audio.side_channels > 0
            && !args.sidechain.is_null();
        let total = audio.events.len();
        let mut cursor = 0usize;
        let mut next = 0usize;
        while next < total {
            let at = audio.events[next].offset as usize;
            let mut group_end = next;
            while group_end < total && audio.events[group_end].offset as usize == at {
                group_end += 1;
            }
            if at > cursor {
                unsafe { self.run_span(audio, args, cursor, at, side) };
                cursor = at;
            }
            self.apply_events(audio, next, group_end);
            next = group_end;
        }
        unsafe { self.run_span(audio, args, cursor, frames, side) };
        self.publish(audio);
        true
    }

    /// Copy up to `capacity` pending MIDI output events to `out`; later calls
    /// continue where this one stopped. Returns the count copied. Zero when a
    /// state call holds the gate. The list is cleared by the next block.
    ///
    /// # Safety
    /// `out` is null or writable for `capacity` events.
    pub unsafe fn pull_midi_out(&self, out: *mut OpMidiEvent, capacity: u32) -> u32 {
        if out.is_null() || capacity == 0 {
            return 0;
        }
        let Some(_gate) = self.enter_audio() else { return 0 };
        let audio = unsafe { &mut *self.audio.get() };
        let pending = audio.midi_out.len().saturating_sub(audio.midi_read);
        let count = pending.min(capacity as usize);
        for i in 0..count {
            unsafe { *out.add(i) = audio.midi_out[audio.midi_read + i] };
        }
        audio.midi_read += count;
        count as u32
    }

    /// MIDI output events lost to a full buffer since the last call.
    pub fn take_midi_dropped(&self) -> u32 {
        self.midi_dropped.swap(0, Ordering::Relaxed)
    }

    /// Select `supported_layouts()[index]` and re-prepare for it: the main
    /// bus width and the sidechain width follow the layout. Allocates; call
    /// while no block runs (a running block makes it wait, at most
    /// [`crate::STATE_WAIT`]). False for an index past the list, a refused
    /// layout, or a timeout; nothing changes then.
    pub fn select_layout(&self, index: u32) -> bool {
        self.with_exclusive(|audio| {
            let Some(layout) = audio.inner.supported_layouts().get(index as usize).copied() else {
                return false;
            };
            if !audio.inner.set_layout(layout) {
                return false;
            }
            let main = layout.main_channels().clamp(1, MAX_AUDIO_CHANNELS);
            let side = layout.sidechain_channels().min(MAX_AUDIO_CHANNELS);
            audio.inner.prepare_layout(audio.rate, audio.max_block, layout);
            audio.buffers = (0..main).map(|_| vec![0.0; audio.max_block]).collect();
            audio.sidechain = (0..side).map(|_| vec![0.0; audio.max_block]).collect();
            audio.channels = main;
            audio.side_channels = side;
            audio.layout_index = index;
            audio.cursor.invalidate();
            audio.cursor.pull(&self.plane, audio.inner.as_mut());
            self.latency
                .store(audio.inner.latency() as u32, Ordering::Release);
            true
        })
        .unwrap_or(false)
    }

    /// Index last given to [`Self::select_layout`], 0 at creation.
    pub fn layout_index(&self) -> u32 {
        self.with_exclusive(|audio| audio.layout_index).unwrap_or(0)
    }

    /// The layout in force, for a caller that wants its channel counts.
    pub fn layout(&self) -> Option<IoConfig> {
        self.with_exclusive(|audio| {
            audio
                .inner
                .supported_layouts()
                .get(audio.layout_index as usize)
                .copied()
        })
        .flatten()
    }
}
