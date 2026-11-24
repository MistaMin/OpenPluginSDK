//! MIDI 1.0 byte parser.
//!
//! Channel and system messages match the set in the MIT `wmidi` crate. This
//! parser is separate code: it reports bytes consumed, keeps running status,
//! and leaves a note-on with velocity 0 as a note-on. Nothing here allocates
//! or panics, so all of it is safe on the audio thread.
//!
//! ```
//! use openplugin_core::midi::{Message, Parser};
//! let bytes = [0x90, 60, 100, 62, 0]; // note-on, then a second one by running status
//! let mut parser = Parser::new();
//! let mut at = 0;
//! let mut notes = 0;
//! while let Ok(Some((message, used))) = parser.pull(&bytes[at..]) {
//!     if let Message::NoteOn { .. } = message { notes += 1; }
//!     at += used;
//! }
//! assert_eq!(notes, 2);
//! ```

use crate::Plugin;

/// 7-bit data value. A status byte does not fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct U7(u8);

impl U7 {
    /// `value` if it is below `0x80`, else `None`.
    ///
    /// ```
    /// use openplugin_core::midi::U7;
    /// assert_eq!(U7::try_new(127).map(U7::get), Some(127));
    /// assert_eq!(U7::try_new(128), None);
    /// ```
    pub const fn try_new(value: u8) -> Option<Self> {
        if value < 0x80 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value, 0..=127.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Channel index 0..15. `number` is the 1-based channel a musician sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel(u8);

impl Channel {
    /// Channel `index` (0..=15), else `None`.
    ///
    /// ```
    /// use openplugin_core::midi::Channel;
    /// let ch = Channel::try_new(9).unwrap();
    /// assert_eq!((ch.index(), ch.number()), (9, 10));
    /// assert!(Channel::try_new(16).is_none());
    /// ```
    pub const fn try_new(index: u8) -> Option<Self> {
        if index < 16 {
            Some(Self(index))
        } else {
            None
        }
    }

    /// 0-based index, 0..=15: the low nibble of the status byte.
    pub const fn index(self) -> u8 {
        self.0
    }

    /// 1-based channel number, 1..=16.
    pub const fn number(self) -> u8 {
        self.0 + 1
    }
}

/// Pitch bend center, per the MIDI 1.0 specification.
pub const PITCH_BEND_CENTER: u16 = 8192;

/// One decoded message. `SysEx` borrows the payload between `F0` and `F7`.
///
/// Data fields are 0..=127 when the [`Parser`] made the message (14-bit
/// values 0..=16383). A message built by hand is written as given:
/// [`Message::write_bytes`] does not mask the fields.
///
/// ```
/// use openplugin_core::midi::{Channel, Message};
/// let cc = Message::ControlChange { channel: Channel::try_new(0).unwrap(), controller: 74, value: 64 };
/// let mut out = [0u8; 3];
/// assert_eq!(cc.write_bytes(&mut out), Some(3));
/// assert_eq!(out, [0xB0, 74, 64]);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Message<'a> {
    /// `8n`: note released.
    NoteOff {
        /// Channel.
        channel: Channel,
        /// Key number, 60 is middle C.
        note: u8,
        /// Release velocity.
        velocity: u8,
    },
    /// `9n`: note pressed. Velocity 0 is kept as a note-on, not turned into a note-off.
    NoteOn {
        /// Channel.
        channel: Channel,
        /// Key number, 60 is middle C.
        note: u8,
        /// Strike velocity; 0 conventionally means note-off.
        velocity: u8,
    },
    /// `An`: polyphonic key pressure (aftertouch).
    PolyPressure {
        /// Channel.
        channel: Channel,
        /// Key number.
        note: u8,
        /// Pressure.
        pressure: u8,
    },
    /// `Bn`: control change.
    ControlChange {
        /// Channel.
        channel: Channel,
        /// Controller number (CC 120..=127 are channel mode messages).
        controller: u8,
        /// Controller value.
        value: u8,
    },
    /// `Cn`: program change.
    ProgramChange {
        /// Channel.
        channel: Channel,
        /// Program number, 0-based.
        program: u8,
    },
    /// `Dn`: channel pressure (aftertouch).
    ChannelPressure {
        /// Channel.
        channel: Channel,
        /// Pressure.
        pressure: u8,
    },
    /// `En`: pitch bend.
    PitchBend {
        /// Channel.
        channel: Channel,
        /// 14-bit bend, 0..=16383; [`PITCH_BEND_CENTER`] is no bend.
        value: u16,
    },
    /// `F0 .. F7`: system exclusive; the payload without `F0` and `F7`.
    SysEx(&'a [u8]),
    /// `F1`: MIDI time code quarter frame.
    QuarterFrame {
        /// The data byte: message type in the high three bits, value in the low four.
        data: u8,
    },
    /// `F2`: song position pointer.
    SongPosition {
        /// 14-bit position in MIDI beats (sixteenth notes) from the start.
        beats: u16,
    },
    /// `F3`: song select.
    SongSelect {
        /// Song number.
        song: u8,
    },
    /// A status byte the specification leaves undefined (`F4`, `F5`, `F9`, `FD`).
    Undefined(u8),
    /// `F6`: tune request.
    TuneRequest,
    /// `F8`: timing clock, 24 per quarter note.
    TimingClock,
    /// `FA`: start.
    Start,
    /// `FB`: continue.
    Continue,
    /// `FC`: stop.
    Stop,
    /// `FE`: active sensing.
    ActiveSensing,
    /// `FF`: system reset.
    SystemReset,
}

/// Why [`Parser::pull`] rejected a buffer. The parser is unchanged after an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MidiError {
    /// The buffer ends before the message's data bytes.
    Truncated,
    /// A data byte is `0x80` or more, or a lone `F7` appears.
    BadData,
    /// The buffer starts with a data byte and there is no running status.
    NoStatus,
    /// A `F0` has no `F7` after it in the buffer.
    UnclosedSysEx,
}

/// Running-status cursor. One owner, no interior allocation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Parser {
    running: Option<u8>,
}

impl Parser {
    /// A parser with no running status.
    pub const fn new() -> Self {
        Self { running: None }
    }

    /// The channel status byte a following data byte would use, if any.
    /// System common messages and SysEx clear it; realtime bytes do not.
    pub const fn running_status(self) -> Option<u8> {
        self.running
    }

    /// Next message at the front of `input`.
    /// `Ok(None)` means the slice is empty. The `usize` is the byte count to advance.
    pub fn pull<'a>(&mut self, input: &'a [u8]) -> Result<Option<(Message<'a>, usize)>, MidiError> {
        let Some((&first, _)) = input.split_first() else {
            return Ok(None);
        };
        if first >= 0xF8 {
            return Ok(Some((realtime(first), 1)));
        }

        let (status, data_at) = if first >= 0x80 {
            (first, 1usize)
        } else {
            match self.running {
                Some(status) => (status, 0usize),
                None => return Err(MidiError::NoStatus),
            }
        };

        let (message, len) = decode(status, input, data_at)?;
        if status < 0xF0 {
            self.running = Some(status);
        } else {
            self.running = None;
        }
        Ok(Some((message, len)))
    }
}

fn realtime(status: u8) -> Message<'static> {
    match status {
        0xF8 => Message::TimingClock,
        0xFA => Message::Start,
        0xFB => Message::Continue,
        0xFC => Message::Stop,
        0xFE => Message::ActiveSensing,
        0xFF => Message::SystemReset,
        other => Message::Undefined(other),
    }
}

fn decode(status: u8, input: &[u8], at: usize) -> Result<(Message<'_>, usize), MidiError> {
    if status == 0xF0 {
        return decode_sysex(input, at);
    }
    let channel = Channel(status & 0x0F);
    match status & 0xF0 {
        0x80 => {
            let (note, velocity, len) = pair(input, at)?;
            Ok((
                Message::NoteOff {
                    channel,
                    note,
                    velocity,
                },
                len,
            ))
        }
        0x90 => {
            let (note, velocity, len) = pair(input, at)?;
            Ok((
                Message::NoteOn {
                    channel,
                    note,
                    velocity,
                },
                len,
            ))
        }
        0xA0 => {
            let (note, pressure, len) = pair(input, at)?;
            Ok((
                Message::PolyPressure {
                    channel,
                    note,
                    pressure,
                },
                len,
            ))
        }
        0xB0 => {
            let (controller, value, len) = pair(input, at)?;
            Ok((
                Message::ControlChange {
                    channel,
                    controller,
                    value,
                },
                len,
            ))
        }
        0xC0 => {
            let (program, len) = one(input, at)?;
            Ok((Message::ProgramChange { channel, program }, len))
        }
        0xD0 => {
            let (pressure, len) = one(input, at)?;
            Ok((Message::ChannelPressure { channel, pressure }, len))
        }
        0xE0 => {
            let (lsb, msb, len) = pair(input, at)?;
            let value = u16::from(lsb) | (u16::from(msb) << 7);
            Ok((Message::PitchBend { channel, value }, len))
        }
        0xF0 => match status {
            0xF1 => {
                let (data, len) = one(input, at)?;
                Ok((Message::QuarterFrame { data }, len))
            }
            0xF2 => {
                let (lsb, msb, len) = pair(input, at)?;
                let beats = u16::from(lsb) | (u16::from(msb) << 7);
                Ok((Message::SongPosition { beats }, len))
            }
            0xF3 => {
                let (song, len) = one(input, at)?;
                Ok((Message::SongSelect { song }, len))
            }
            0xF4 | 0xF5 => Ok((Message::Undefined(status), at)),
            0xF6 => Ok((Message::TuneRequest, at)),
            0xF7 => Err(MidiError::BadData),
            _ => Ok((realtime(status), at)),
        },
        _ => Err(MidiError::BadData),
    }
}

fn decode_sysex(input: &[u8], at: usize) -> Result<(Message<'_>, usize), MidiError> {
    let rest = input.get(at..).ok_or(MidiError::UnclosedSysEx)?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0xF7)
        .ok_or(MidiError::UnclosedSysEx)?;
    if rest[..end].iter().any(|byte| *byte >= 0x80) {
        return Err(MidiError::BadData);
    }
    Ok((Message::SysEx(&rest[..end]), at + end + 1))
}

fn one(input: &[u8], at: usize) -> Result<(u8, usize), MidiError> {
    let byte = *input.get(at).ok_or(MidiError::Truncated)?;
    if byte >= 0x80 {
        return Err(MidiError::BadData);
    }
    Ok((byte, at + 1))
}

fn pair(input: &[u8], at: usize) -> Result<(u8, u8, usize), MidiError> {
    let (first, len) = one(input, at)?;
    let (second, len) = one(input, at + 1).map(|(b, _)| (b, len + 1))?;
    Ok((first, second, len))
}

impl Message<'_> {
    /// Write the wire bytes. `SysEx` is `F0`, payload, `F7`.
    /// Returns `None` when `out` is shorter than the message.
    pub fn write_bytes(&self, out: &mut [u8]) -> Option<usize> {
        let bytes = match *self {
            Message::NoteOff {
                channel,
                note,
                velocity,
            } => [0x80 | channel.index(), note, velocity],
            Message::NoteOn {
                channel,
                note,
                velocity,
            } => [0x90 | channel.index(), note, velocity],
            Message::PolyPressure {
                channel,
                note,
                pressure,
            } => [0xA0 | channel.index(), note, pressure],
            Message::ControlChange {
                channel,
                controller,
                value,
            } => [0xB0 | channel.index(), controller, value],
            Message::ProgramChange { channel, program } => [0xC0 | channel.index(), program, 0],
            Message::ChannelPressure { channel, pressure } => [0xD0 | channel.index(), pressure, 0],
            Message::PitchBend { channel, value } => [
                0xE0 | channel.index(),
                (value & 0x7F) as u8,
                (value >> 7) as u8,
            ],
            Message::QuarterFrame { data } => [0xF1, data, 0],
            Message::SongPosition { beats } => [0xF2, (beats & 0x7F) as u8, (beats >> 7) as u8],
            Message::SongSelect { song } => [0xF3, song, 0],
            Message::Undefined(status) => [status, 0, 0],
            Message::TuneRequest => [0xF6, 0, 0],
            Message::TimingClock => [0xF8, 0, 0],
            Message::Start => [0xFA, 0, 0],
            Message::Continue => [0xFB, 0, 0],
            Message::Stop => [0xFC, 0, 0],
            Message::ActiveSensing => [0xFE, 0, 0],
            Message::SystemReset => [0xFF, 0, 0],
            Message::SysEx(payload) => {
                let len = payload.len().checked_add(2)?;
                if out.len() < len {
                    return None;
                }
                out[0] = 0xF0;
                out[1..payload.len() + 1].copy_from_slice(payload);
                out[payload.len() + 1] = 0xF7;
                return Some(len);
            }
        };
        let len = wire_len(*self);
        if out.len() < len {
            return None;
        }
        out[..len].copy_from_slice(&bytes[..len]);
        Some(len)
    }
}

fn wire_len(message: Message<'_>) -> usize {
    match message {
        Message::ProgramChange { .. }
        | Message::ChannelPressure { .. }
        | Message::QuarterFrame { .. }
        | Message::SongSelect { .. } => 2,
        Message::Undefined(_)
        | Message::TuneRequest
        | Message::TimingClock
        | Message::Start
        | Message::Continue
        | Message::Stop
        | Message::ActiveSensing
        | Message::SystemReset => 1,
        Message::SysEx(payload) => payload.len().saturating_add(2),
        _ => 3,
    }
}

/// Send note-on and note-off through the trait. Every other message that fits
/// in three bytes goes to [`Plugin::midi`]. Release velocity stays on the
/// [`Message::NoteOff`] value because `note_off` has no velocity argument.
pub fn deliver(plugin: &mut dyn Plugin, sample: usize, message: Message<'_>) {
    match message {
        Message::NoteOn { note, velocity, .. } => plugin.note_on(sample, note, velocity),
        Message::NoteOff { note, .. } => plugin.note_off(sample, note),
        other => {
            let mut bytes = [0u8; 3];
            if other.write_bytes(&mut bytes).is_some() {
                plugin.midi(sample, bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_velocity_zero_stays_note_on_and_running_status_advances() {
        let mut parser = Parser::new();
        let bytes = [0x90, 60, 0, 62, 100, 0xF8, 64, 40];
        let (first, n) = parser.pull(&bytes).unwrap().unwrap();
        assert_eq!(
            first,
            Message::NoteOn {
                channel: Channel(0),
                note: 60,
                velocity: 0
            }
        );
        assert_eq!(n, 3);
        let (second, n) = parser.pull(&bytes[3..]).unwrap().unwrap();
        assert_eq!(
            second,
            Message::NoteOn {
                channel: Channel(0),
                note: 62,
                velocity: 100
            }
        );
        assert_eq!(n, 2);
        let (clock, n) = parser.pull(&bytes[5..]).unwrap().unwrap();
        assert_eq!(clock, Message::TimingClock);
        assert_eq!(n, 1);
        assert_eq!(parser.running_status(), Some(0x90));
        let (third, _) = parser.pull(&bytes[6..]).unwrap().unwrap();
        assert_eq!(
            third,
            Message::NoteOn {
                channel: Channel(0),
                note: 64,
                velocity: 40
            }
        );
    }

    #[test]
    fn pitch_bend_center_and_sysex_round_trip() {
        let mut parser = Parser::new();
        let bend = [0xE0, 0x00, 0x40];
        let (message, n) = parser.pull(&bend).unwrap().unwrap();
        assert_eq!(n, 3);
        assert_eq!(
            message,
            Message::PitchBend {
                channel: Channel(0),
                value: PITCH_BEND_CENTER
            }
        );
        let mut out = [0u8; 3];
        assert_eq!(message.write_bytes(&mut out), Some(3));
        assert_eq!(out, bend);

        let sysex = [0xF0, 0x7E, 0x00, 0xF7];
        let (message, n) = parser.pull(&sysex).unwrap().unwrap();
        assert_eq!(n, 4);
        assert_eq!(message, Message::SysEx(&[0x7E, 0x00]));
        assert_eq!(parser.running_status(), None);
        assert!(parser.pull(&[0xF0, 0x01]).unwrap_err() == MidiError::UnclosedSysEx);
        assert_eq!(parser.running_status(), None);
    }

    #[test]
    fn truncated_channel_message_does_not_stick_running_status() {
        let mut parser = Parser::new();
        assert_eq!(parser.pull(&[0x90, 60]).unwrap_err(), MidiError::Truncated);
        assert_eq!(parser.running_status(), None);
        assert_eq!(parser.pull(&[12, 40]).unwrap_err(), MidiError::NoStatus);
    }
}
