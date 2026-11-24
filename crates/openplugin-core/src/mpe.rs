//! MPE (MIDI Polyphonic Expression) over MIDI 1.0, turned into per-note events.
//!
//! Written from the MMA/AMEI Recommended Practice RP-053, *MIDI Polyphonic
//! Expression*, version 1.0 (12 March 2018); version 1.1 (M1-100-UM) changes
//! the wording, not the design. Section numbers below are those of 1.0.
//!
//! Every format entry that carries MIDI (CLAP's MIDI dialect, LV2, and the
//! VST3, Audio Unit and AAX shells through `op_shell_process_events`) feeds a
//! plugin that asks for MPE ([`Plugin::mpe_input`])
//! through one [`MpeReceiver`]. A plugin that does not ask gets plain MIDI as
//! before. The receiver turns MIDI into note-ons and note-offs with a
//! [`NoteRef`] and [`Expression`] events, and passes everything else on as raw
//! MIDI:
//!
//! - **Zones** (2.1.1): the MPE Configuration Message (MCM), RPN 6 (`Bn 64 06`,
//!   `Bn 65 00`, `Bn 06 mm`) on channel 1 sets the Lower Zone to `mm` member
//!   channels from channel 2 up, on channel 16 the Upper Zone from channel 15
//!   down; `mm` 0 turns the zone off. The master channel of an unused zone may
//!   be a member of the other, so one zone can have 15 members. A channel
//!   belongs to one zone: the most recent MCM wins, shrinking or turning off
//!   the other zone. MCMs on other channels are ignored.
//! - **Resetting zones** (2.1.4): every channel that enters or leaves a zone
//!   has its notes stopped (a note-off at velocity 64) and its controls reset.
//! - **Pitch bend** (2.4): an MCM sets the member channels' pitch bend range
//!   to ±48 semitones and the master's to ±2. RPN 0 (pitch bend sensitivity,
//!   MSB semitones, LSB cents, at most 96) on the master channel sets the
//!   master range; on any member channel it sets the range of every member of
//!   the zone. A note's tuning is its channel's bend times the member range
//!   plus the master's bend times the master range (2.3.2, 2.4: the two
//!   combine), sent as [`Expression::Tuning`].
//! - **Pressure and timbre** (2.5, 2.6): channel pressure is
//!   [`Expression::Pressure`], CC 74 [`Expression::Brightness`], each
//!   `value / 127`. The master channel's value combines with the member's: the
//!   member value plus the master's offset from its rest value (pressure 0,
//!   CC 74 64, the *Initial-64* centre of 3.3.5), clamped to 0..1.
//! - **Initial state** (3.3): bend, pressure and CC 74 received on a channel
//!   before its note-on set the new note's starting values, so each note-on is
//!   followed at once by its three expressions. A member channel's messages
//!   stop reaching a note after its note-off; the master's pitch bend reaches
//!   every note of the zone, released ones included (3.3.3).
//! - **Note-on with velocity 0** on a zone channel is a note-off with velocity
//!   64 (3.3.2).
//! - **Zone messages** (2.3.1, Table 1): on a member channel, controllers other
//!   than CC 74 and the RPN ones, program change and polyphonic key pressure are
//!   ignored; on the master channel they pass through as raw MIDI. Polyphonic
//!   key pressure on the master channel is that note's pressure (2.5).
//! - **Channels in no zone** (2.1.1, "remain available for conventional use")
//!   are plain MIDI: their notes are note-ons and note-offs with a channel and
//!   no expression, everything else is raw MIDI. With no zone at all the
//!   whole stream is plain MIDI.
//!
//! The receiver keeps fixed arrays (16 channels, a 128-bit set of keys per
//! channel); it never allocates and never panics on any byte, so it runs on
//! the audio thread.
//!
//! ```
//! use openplugin_core::expression::Expression;
//! use openplugin_core::mpe::{MpeEvent, MpeReceiver, MpeZones};
//! let mut mpe = MpeReceiver::new(MpeZones::LOWER_15);
//! let mut events = Vec::new();
//! mpe.receive([0x91, 60, 100], |e| events.push(e)); // note-on, member channel 2
//! mpe.receive([0xE1, 0x00, 0x60], |e| events.push(e)); // bend up on that channel
//! assert!(matches!(events[0], MpeEvent::NoteOn { velocity: 100, .. }));
//! let tuning = events.iter().rev().find_map(|e| match e {
//!     MpeEvent::Expression { expression: Expression::Tuning, value, .. } => Some(*value),
//!     _ => None,
//! });
//! assert!(tuning.unwrap() > 23.0); // half of the 48-semitone member range
//! ```

use crate::expression::{Expression, NoteRef};
use crate::Plugin;

/// Member channel counts of the two zones (2.1.1). `0` turns a zone off.
///
/// ```
/// use openplugin_core::mpe::{MpeZones, Zone};
/// let zones = MpeZones::new(7, 7);
/// assert_eq!(zones.zone_of(0).map(|z| z.master), Some(true));
/// assert_eq!(zones.zone_of(8).map(|z| z.zone), Some(Zone::Upper));
/// assert_eq!(MpeZones::new(15, 15).upper(), 0); // the Lower Zone keeps its channels
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MpeZones {
    lower: u8,
    upper: u8,
}

impl MpeZones {
    /// No zone: plain MIDI until an MCM arrives.
    pub const NONE: Self = Self { lower: 0, upper: 0 };
    /// The recommended power-on layout (2.1.3): the Lower Zone with member
    /// channels 2 to 16.
    pub const LOWER_15: Self = Self { lower: 15, upper: 0 };

    /// `lower` and `upper` member channels (each at most 15). Where they
    /// would overlap the Lower Zone keeps its channels and the Upper Zone
    /// shrinks, or turns off.
    pub const fn new(lower: u8, upper: u8) -> Self {
        let lower = if lower > 15 { 15 } else { lower };
        let mut upper = if upper > 15 { 15 } else { upper };
        if lower > 0 && upper > 0 && lower + upper > 14 {
            upper = 14u8.saturating_sub(lower);
        }
        Self { lower, upper }
    }

    /// Member channels of the Lower Zone, 0..=15.
    pub const fn lower(self) -> u8 {
        self.lower
    }

    /// Member channels of the Upper Zone, 0..=15.
    pub const fn upper(self) -> u8 {
        self.upper
    }

    /// The zone channel `channel` (0..=15) belongs to, if any.
    pub const fn zone_of(self, channel: u8) -> Option<ZoneChannel> {
        if channel > 15 {
            return None;
        }
        if self.lower > 0 {
            if channel == 0 {
                return Some(ZoneChannel { zone: Zone::Lower, master: true });
            }
            if channel <= self.lower {
                return Some(ZoneChannel { zone: Zone::Lower, master: false });
            }
        }
        if self.upper > 0 {
            if channel == 15 {
                return Some(ZoneChannel { zone: Zone::Upper, master: true });
            }
            if channel >= 15 - self.upper {
                return Some(ZoneChannel { zone: Zone::Upper, master: false });
            }
        }
        None
    }
}

impl Default for MpeZones {
    fn default() -> Self {
        Self::LOWER_15
    }
}

/// Lower (master channel 1) or Upper (master channel 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Master channel 1, members counting up from channel 2.
    Lower,
    /// Master channel 16, members counting down from channel 15.
    Upper,
}

impl Zone {
    const fn index(self) -> usize {
        match self {
            Self::Lower => 0,
            Self::Upper => 1,
        }
    }

    /// Master channel, 0-based (0 or 15).
    pub const fn master(self) -> u8 {
        match self {
            Self::Lower => 0,
            Self::Upper => 15,
        }
    }
}

/// Where a channel sits in a zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZoneChannel {
    /// The zone.
    pub zone: Zone,
    /// The zone's master channel, rather than a member.
    pub master: bool,
}

/// What the receiver hands on, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum MpeEvent {
    /// A note started.
    NoteOn {
        /// The note: port 0, its channel and key, and the id given to
        /// [`MpeReceiver::receive_with_id`] (or -1).
        note: NoteRef,
        /// Strike velocity, 0..=127.
        velocity: u8,
    },
    /// A note ended.
    NoteOff {
        /// The note, as at its note-on.
        note: NoteRef,
        /// Release velocity, 0..=127 (64 for a note-on with velocity 0 or a zone reset).
        velocity: u8,
    },
    /// A per-note expression changed.
    Expression {
        /// The note it is for.
        note: NoteRef,
        /// Which expression: [`Expression::Tuning`], [`Expression::Pressure`] or [`Expression::Brightness`].
        expression: Expression,
        /// Semitones for tuning; 0..=1 for pressure and brightness.
        value: f32,
    },
    /// A message the receiver does not turn into note events, as received.
    Midi([u8; 3]),
}

/// Member pitch bend range an MCM sets, in semitones (2.4).
pub const MEMBER_BEND_RANGE: f32 = 48.0;
/// Master pitch bend range an MCM sets, in semitones (2.4).
pub const MASTER_BEND_RANGE: f32 = 2.0;
/// Largest pitch bend range MPE allows, in semitones (2.4).
pub const MAX_BEND_RANGE: f32 = 96.0;
/// CC 74's rest value, 64 of 127 (3.3.5, *Initial-64*).
pub const TIMBRE_REST: f32 = 64.0 / 127.0;

const RPN_NULL: u8 = 127;

#[derive(Clone, Copy, Debug)]
struct ChannelState {
    /// Pitch bend, -1..=1.
    bend: f32,
    pressure: f32,
    timbre: f32,
    rpn_msb: u8,
    rpn_lsb: u8,
}

impl ChannelState {
    const REST: Self = Self { bend: 0.0, pressure: 0.0, timbre: TIMBRE_REST, rpn_msb: RPN_NULL, rpn_lsb: RPN_NULL };
}

/// Turns an MPE MIDI stream into note and expression events. See the module
/// documentation for the rules.
#[derive(Clone, Debug)]
pub struct MpeReceiver {
    zones: MpeZones,
    member_range: [f32; 2],
    master_range: [f32; 2],
    channels: [ChannelState; 16],
    /// Keys held on each channel (note-on seen, note-off not yet).
    active: [u128; 16],
}

impl Default for MpeReceiver {
    fn default() -> Self {
        Self::new(MpeZones::default())
    }
}

/// 14-bit pitch bend to -1..=1: the centre 8192 is 0, 0 is -1 and 16383 is +1.
pub fn bend_unit(value: u16) -> f32 {
    let v = f32::from(value.min(16383)) - 8192.0;
    if v < 0.0 { v / 8192.0 } else { v / 8191.0 }
}

impl MpeReceiver {
    /// A receiver with these zones and the ranges an MCM would set.
    pub const fn new(zones: MpeZones) -> Self {
        Self {
            zones,
            member_range: [MEMBER_BEND_RANGE; 2],
            master_range: [MASTER_BEND_RANGE; 2],
            channels: [ChannelState::REST; 16],
            active: [0; 16],
        }
    }

    /// The receiver a plugin asks for, or `None` for plain MIDI.
    pub fn for_plugin(plugin: &dyn Plugin) -> Option<Self> {
        plugin.mpe_input().map(Self::new)
    }

    /// The zones in force: the starting ones, as changed by any MCM since.
    pub const fn zones(&self) -> MpeZones {
        self.zones
    }

    /// Member pitch bend range of a zone, in semitones.
    pub const fn member_bend_range(&self, zone: Zone) -> f32 {
        self.member_range[zone.index()]
    }

    /// Master pitch bend range of a zone, in semitones.
    pub const fn master_bend_range(&self, zone: Zone) -> f32 {
        self.master_range[zone.index()]
    }

    /// Whether `key` is held on `channel`.
    pub fn is_active(&self, channel: u8, key: u8) -> bool {
        channel < 16 && key < 128 && self.active[channel as usize] & (1u128 << key) != 0
    }

    /// Forget every held note and reset every channel's controls; the zones
    /// and ranges stay. For a transport reset or all-notes-off.
    pub fn reset(&mut self) {
        self.channels = [ChannelState::REST; 16];
        self.active = [0; 16];
    }

    /// Takes one MIDI 1.0 message (status and two data bytes; unused bytes
    /// are ignored) and calls `emit` for each event it becomes, in order.
    /// A data byte with the top bit set makes the message plain MIDI, as received.
    pub fn receive(&mut self, bytes: [u8; 3], emit: impl FnMut(MpeEvent)) {
        self.receive_with_id(bytes, -1, emit);
    }

    /// [`Self::receive`] for a note-on or note-off that came with the host's
    /// note id (a VST3 note whose pitch bend arrives as MIDI): the note's
    /// [`NoteRef`] carries the id, so both the id and the channel reach its
    /// voice. `id` is ignored for other messages; below 0 is none.
    pub fn receive_with_id(&mut self, bytes: [u8; 3], id: i32, mut emit: impl FnMut(MpeEvent)) {
        let status = bytes[0];
        if status < 0x80 {
            return;
        }
        if status >= 0xF0 || bytes[1] >= 0x80 || bytes[2] >= 0x80 {
            emit(MpeEvent::Midi(bytes));
            return;
        }
        let channel = status & 0x0F;
        let kind = status & 0xF0;
        // The MCM is read on channels 1 and 16 whatever zone they are in now.
        if kind == 0xB0 && self.rpn(channel, bytes[1], bytes[2], &mut emit) {
            return;
        }
        let Some(place) = self.zones.zone_of(channel) else {
            self.plain(channel, bytes, id, &mut emit);
            return;
        };
        let zone = place.zone;
        let master = zone.master();
        let at = channel as usize;
        match kind {
            0x90 if bytes[2] > 0 => {
                self.active[channel as usize] |= 1u128 << bytes[1];
                let note = NoteRef::midi(channel, bytes[1]).with_id(id);
                emit(MpeEvent::NoteOn { note, velocity: bytes[2] });
                self.initial(channel, note, zone, place.master, &mut emit);
            }
            0x80 | 0x90 => {
                self.active[channel as usize] &= !(1u128 << bytes[1]);
                let velocity = if kind == 0x90 { 64 } else { bytes[2] };
                emit(MpeEvent::NoteOff { note: NoteRef::midi(channel, bytes[1]).with_id(id), velocity });
            }
            0xE0 => {
                self.channels[at].bend = bend_unit(u16::from(bytes[1]) | (u16::from(bytes[2]) << 7));
                if place.master {
                    // The master's bend reaches every note of the zone, released ones too (3.3.3).
                    for c in 0..16u8 {
                        if self.zones.zone_of(c).is_some_and(|p| p.zone == zone) {
                            let value = self.tuning(c, zone, c == master);
                            emit(MpeEvent::Expression { note: NoteRef::on_channel(c), expression: Expression::Tuning, value });
                        }
                    }
                } else {
                    let value = self.tuning(channel, zone, false);
                    self.each_key(channel, |note| emit(MpeEvent::Expression { note, expression: Expression::Tuning, value }));
                }
            }
            0xD0 => {
                self.channels[at].pressure = f32::from(bytes[1]) / 127.0;
                self.dimension(channel, zone, place.master, Expression::Pressure, &mut emit);
            }
            0xB0 if bytes[1] == 74 => {
                self.channels[at].timbre = f32::from(bytes[2]) / 127.0;
                self.dimension(channel, zone, place.master, Expression::Brightness, &mut emit);
            }
            0xA0 if place.master => {
                let note = NoteRef::midi(channel, bytes[1]);
                if self.is_active(channel, bytes[1]) {
                    emit(MpeEvent::Expression { note, expression: Expression::Pressure, value: f32::from(bytes[2]) / 127.0 });
                }
            }
            // Zone messages on the master pass through; on a member they are ignored (2.3.1).
            _ if place.master => emit(MpeEvent::Midi(bytes)),
            _ => {}
        }
    }

    /// A channel in no zone: plain MIDI.
    fn plain(&mut self, channel: u8, bytes: [u8; 3], id: i32, emit: &mut impl FnMut(MpeEvent)) {
        let note = NoteRef::midi(channel, bytes[1]).with_id(id);
        match bytes[0] & 0xF0 {
            0x90 => emit(MpeEvent::NoteOn { note, velocity: bytes[2] }),
            0x80 => emit(MpeEvent::NoteOff { note, velocity: bytes[2] }),
            _ => emit(MpeEvent::Midi(bytes)),
        }
    }

    /// The tuning of a note on `channel` in semitones.
    fn tuning(&self, channel: u8, zone: Zone, on_master: bool) -> f32 {
        let z = zone.index();
        let master = self.channels[zone.master() as usize].bend * self.master_range[z];
        if on_master {
            master
        } else {
            self.channels[channel as usize].bend * self.member_range[z] + master
        }
    }

    /// Pressure or brightness of a note on `channel`: its own value plus the
    /// master's offset from rest.
    fn combined(&self, channel: u8, zone: Zone, on_master: bool, expression: Expression) -> f32 {
        let pick = |c: &ChannelState| match expression {
            Expression::Pressure => (c.pressure, 0.0),
            _ => (c.timbre, TIMBRE_REST),
        };
        let (master, rest) = pick(&self.channels[zone.master() as usize]);
        if on_master {
            return master;
        }
        let (own, _) = pick(&self.channels[channel as usize]);
        (own + master - rest).clamp(0.0, 1.0)
    }

    fn dimension(&self, channel: u8, zone: Zone, master: bool, expression: Expression, emit: &mut impl FnMut(MpeEvent)) {
        let mut send = |c: u8| {
            let value = self.combined(c, zone, c == zone.master(), expression);
            self.each_key(c, |note| emit(MpeEvent::Expression { note, expression, value }));
        };
        if master {
            for c in 0..16u8 {
                if self.zones.zone_of(c).is_some_and(|p| p.zone == zone) {
                    send(c);
                }
            }
        } else {
            send(channel);
        }
    }

    fn initial(&self, channel: u8, note: NoteRef, zone: Zone, master: bool, emit: &mut impl FnMut(MpeEvent)) {
        emit(MpeEvent::Expression { note, expression: Expression::Tuning, value: self.tuning(channel, zone, master) });
        for expression in [Expression::Pressure, Expression::Brightness] {
            emit(MpeEvent::Expression { note, expression, value: self.combined(channel, zone, master, expression) });
        }
    }

    fn each_key(&self, channel: u8, mut f: impl FnMut(NoteRef)) {
        let mut keys = self.active[channel as usize];
        while keys != 0 {
            let key = keys.trailing_zeros() as u8;
            keys &= keys - 1;
            f(NoteRef::midi(channel, key));
        }
    }

    /// The RPN controllers. True when the message was taken here.
    fn rpn(&mut self, channel: u8, controller: u8, value: u8, emit: &mut impl FnMut(MpeEvent)) -> bool {
        let place = self.zones.zone_of(channel);
        let member = place.is_some_and(|p| !p.master);
        let state = &mut self.channels[channel as usize];
        match controller {
            101 => state.rpn_msb = value,
            100 => state.rpn_lsb = value,
            99 | 98 => {
                // An NRPN deselects the RPN, so later data entry is not ours.
                state.rpn_msb = RPN_NULL;
                state.rpn_lsb = RPN_NULL;
            }
            6 => match (state.rpn_msb, state.rpn_lsb) {
                (0, 6) if channel == 0 || channel == 15 => {
                    self.configure(if channel == 0 { Zone::Lower } else { Zone::Upper }, value, emit);
                    return true;
                }
                (0, 0) if place.is_some() => {
                    self.set_range(channel, f32::from(value));
                    return true;
                }
                _ => {}
            },
            38 => {
                if (state.rpn_msb, state.rpn_lsb) == (0, 0) && place.is_some() {
                    let semitones = self.range_of(channel).floor();
                    self.set_range(channel, semitones + f32::from(value) / 100.0);
                    return true;
                }
            }
            _ => return false,
        }
        // RPN selection on a member channel is note level only (Table 1); elsewhere it passes on.
        if !member {
            emit(MpeEvent::Midi([0xB0 | channel, controller, value]));
        }
        true
    }

    fn range_of(&self, channel: u8) -> f32 {
        match self.zones.zone_of(channel) {
            Some(p) if p.master => self.master_range[p.zone.index()],
            Some(p) => self.member_range[p.zone.index()],
            None => 0.0,
        }
    }

    fn set_range(&mut self, channel: u8, semitones: f32) {
        let semitones = semitones.clamp(0.0, MAX_BEND_RANGE);
        match self.zones.zone_of(channel) {
            Some(p) if p.master => self.master_range[p.zone.index()] = semitones,
            // Any member sets every member of the zone (2.4).
            Some(p) => self.member_range[p.zone.index()] = semitones,
            None => {}
        }
    }

    /// An MCM for `zone` with `members` channels (2.1.1, 2.1.4, 2.4).
    fn configure(&mut self, zone: Zone, members: u8, emit: &mut impl FnMut(MpeEvent)) {
        let members = members.min(15);
        let before = self.zones;
        let after = match zone {
            // The newest message wins: the other zone gives up the channels.
            Zone::Lower => {
                let upper = if members == 0 { before.upper } else { before.upper.min(14u8.saturating_sub(members)) };
                MpeZones { lower: members, upper }
            }
            Zone::Upper => {
                let lower = if members == 0 { before.lower } else { before.lower.min(14u8.saturating_sub(members)) };
                MpeZones { lower, upper: members }
            }
        };
        for c in 0..16u8 {
            let old = before.zone_of(c);
            let new = after.zone_of(c);
            let touched = old != new || old.is_some_and(|p| p.zone == zone);
            if !touched {
                continue;
            }
            let keys = self.active[c as usize];
            self.active[c as usize] = 0;
            let mut left = keys;
            while left != 0 {
                let key = left.trailing_zeros() as u8;
                left &= left - 1;
                emit(MpeEvent::NoteOff { note: NoteRef::midi(c, key), velocity: 64 });
            }
            self.channels[c as usize] = ChannelState::REST;
        }
        self.zones = after;
        self.member_range[zone.index()] = MEMBER_BEND_RANGE;
        self.master_range[zone.index()] = MASTER_BEND_RANGE;
    }
}

/// Hands one receiver event to the plugin at block sample `offset`.
pub fn deliver(plugin: &mut dyn Plugin, offset: usize, event: MpeEvent) {
    match event {
        MpeEvent::NoteOn { note, velocity } => plugin.note_on_ref(offset, note, velocity),
        MpeEvent::NoteOff { note, velocity } => plugin.note_off_ref(offset, note, velocity),
        MpeEvent::Expression { note, expression, value } => plugin.note_expression(offset, note, expression, value),
        MpeEvent::Midi(bytes) => plugin.midi(offset, bytes),
    }
}

/// [`MpeReceiver::receive`] straight into a plugin, at block sample `offset`.
pub fn receive_into(receiver: &mut MpeReceiver, plugin: &mut dyn Plugin, offset: usize, bytes: [u8; 3]) {
    receiver.receive(bytes, |event| deliver(plugin, offset, event));
}

/// [`MpeReceiver::receive_with_id`] straight into a plugin.
pub fn receive_note_into(receiver: &mut MpeReceiver, plugin: &mut dyn Plugin, offset: usize, bytes: [u8; 3], id: i32) {
    receiver.receive_with_id(bytes, id, |event| deliver(plugin, offset, event));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rx: &mut MpeReceiver, messages: &[[u8; 3]]) -> Vec<MpeEvent> {
        let mut out = Vec::new();
        for m in messages {
            rx.receive(*m, |e| out.push(e));
        }
        out
    }

    fn mcm(channel: u8, members: u8) -> [[u8; 3]; 3] {
        [[0xB0 | channel, 101, 0], [0xB0 | channel, 100, 6], [0xB0 | channel, 6, members]]
    }

    fn tuning_of(events: &[MpeEvent], note: NoteRef) -> Option<f32> {
        events.iter().rev().find_map(|e| match *e {
            MpeEvent::Expression { note: n, expression: Expression::Tuning, value } if n == note => Some(value),
            _ => None,
        })
    }

    #[test]
    fn the_mcm_configures_each_zone_and_the_newest_wins() {
        let mut rx = MpeReceiver::new(MpeZones::NONE);
        run(&mut rx, &mcm(0, 7));
        assert_eq!(rx.zones(), MpeZones::new(7, 0));
        run(&mut rx, &mcm(15, 7));
        assert_eq!((rx.zones().lower(), rx.zones().upper()), (7, 7));
        assert_eq!(rx.zones().zone_of(7), Some(ZoneChannel { zone: Zone::Lower, master: false }));
        assert_eq!(rx.zones().zone_of(8), Some(ZoneChannel { zone: Zone::Upper, master: false }));
        assert_eq!(rx.zones().zone_of(15), Some(ZoneChannel { zone: Zone::Upper, master: true }));
        // A Lower Zone of 10 takes channels 2..11; the Upper Zone shrinks to 4.
        run(&mut rx, &mcm(0, 10));
        assert_eq!((rx.zones().lower(), rx.zones().upper()), (10, 4));
        // An Upper Zone of 15 turns the Lower Zone off; channel 1 is then a member.
        run(&mut rx, &mcm(15, 15));
        assert_eq!((rx.zones().lower(), rx.zones().upper()), (0, 15));
        assert_eq!(rx.zones().zone_of(0), Some(ZoneChannel { zone: Zone::Upper, master: false }));
        // An MCM on another channel is ignored (and is plain MIDI there).
        let out = run(&mut rx, &mcm(3, 5));
        assert_eq!((rx.zones().lower(), rx.zones().upper()), (0, 15));
        assert!(out.is_empty(), "channel 4 is an upper member; its RPN controllers are note level");
        // mm = 0 turns a zone off.
        run(&mut rx, &mcm(15, 0));
        assert_eq!(rx.zones(), MpeZones::NONE);
        assert_eq!(MpeZones::new(15, 3), MpeZones::new(15, 0));
    }

    #[test]
    fn member_bend_uses_the_member_range_and_the_master_adds() {
        let mut rx = MpeReceiver::new(MpeZones::LOWER_15);
        let out = run(&mut rx, &[[0x92, 60, 100], [0x93, 64, 100]]);
        let a = NoteRef::midi(2, 60);
        let b = NoteRef::midi(3, 64);
        assert!(out.contains(&MpeEvent::NoteOn { note: a, velocity: 100 }));
        assert_eq!(tuning_of(&out, a), Some(0.0));
        // Full bend up on channel 3: +48 semitones for that note only.
        let out = run(&mut rx, &[[0xE2, 0x7F, 0x7F]]);
        assert_eq!(tuning_of(&out, a), Some(48.0));
        assert_eq!(tuning_of(&out, b), None);
        // The master (channel 1) bends every note of the zone by its own ±2 range.
        let out = run(&mut rx, &[[0xE0, 0x00, 0x00]]);
        assert_eq!(tuning_of(&out, NoteRef::on_channel(2)), Some(46.0));
        assert_eq!(tuning_of(&out, NoteRef::on_channel(3)), Some(-2.0));
        assert!(tuning_of(&out, NoteRef::on_channel(9)).is_some(), "every channel of the zone");
    }

    #[test]
    fn rpn_zero_on_one_member_sets_every_member_and_the_mcm_resets_it() {
        let mut rx = MpeReceiver::new(MpeZones::NONE);
        run(&mut rx, &mcm(0, 15));
        assert_eq!(rx.member_bend_range(Zone::Lower), 48.0);
        assert_eq!(rx.master_bend_range(Zone::Lower), 2.0);
        // RPN 0 on member channel 5: 12 semitones and 50 cents for all members.
        run(&mut rx, &[[0xB4, 101, 0], [0xB4, 100, 0], [0xB4, 6, 12], [0xB4, 38, 50]]);
        assert_eq!(rx.member_bend_range(Zone::Lower), 12.5);
        let out = run(&mut rx, &[[0x96, 60, 90], [0xE6, 0x7F, 0x7F]]);
        assert_eq!(tuning_of(&out, NoteRef::midi(6, 60)), Some(12.5));
        // RPN 0 on the master sets the master range only.
        run(&mut rx, &[[0xB0, 101, 0], [0xB0, 100, 0], [0xB0, 6, 7]]);
        assert_eq!(rx.master_bend_range(Zone::Lower), 7.0);
        assert_eq!(rx.member_bend_range(Zone::Lower), 12.5);
        // Clamped to ±96.
        run(&mut rx, &[[0xB0, 6, 127]]);
        assert_eq!(rx.master_bend_range(Zone::Lower), 96.0);
        // A new MCM restores ±48 and ±2, and stops the note on a channel it resets.
        let out = run(&mut rx, &mcm(0, 15));
        assert!(out.contains(&MpeEvent::NoteOff { note: NoteRef::midi(6, 60), velocity: 64 }));
        assert_eq!((rx.member_bend_range(Zone::Lower), rx.master_bend_range(Zone::Lower)), (48.0, 2.0));
    }

    #[test]
    fn state_before_note_on_sets_the_initial_values_and_master_messages_reach_the_zone() {
        let mut rx = MpeReceiver::new(MpeZones::LOWER_15);
        // The order of 3.3.1 on channel 3: a quarter tone up (8277 at ±48), CC 74 = 64, pressure 0.
        let out = run(&mut rx, &[[0xE2, 0x55, 0x40], [0xB2, 0x4A, 0x40], [0xD2, 0x00, 0], [0x92, 0x3C, 0x38]]);
        let note = NoteRef::midi(2, 60);
        let t = tuning_of(&out, note).unwrap();
        assert!((t - 0.5).abs() < 0.01, "quarter tone: {t}");
        assert!(out.contains(&MpeEvent::Expression { note, expression: Expression::Brightness, value: 64.0 / 127.0 }));
        assert!(out.contains(&MpeEvent::Expression { note, expression: Expression::Pressure, value: 0.0 }));
        // Pressure on the master adds to every held note of the zone.
        run(&mut rx, &[[0x93, 64, 100]]);
        let out = run(&mut rx, &[[0xD0, 127, 0]]);
        let pressured: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                MpeEvent::Expression { note, expression: Expression::Pressure, value } => Some((*note, *value)),
                _ => None,
            })
            .collect();
        assert_eq!(pressured, vec![(note, 1.0), (NoteRef::midi(3, 64), 1.0)]);
        // A member CC 74 reaches only its own note; zone messages on a member are ignored.
        let out = run(&mut rx, &[[0xB3, 74, 0], [0xB3, 64, 127], [0xC3, 5, 0]]);
        assert_eq!(out.len(), 1);
        // On the master they pass through.
        let out = run(&mut rx, &[[0xB0, 64, 127]]);
        assert_eq!(out, vec![MpeEvent::Midi([0xB0, 64, 127])]);
    }

    #[test]
    fn note_on_velocity_zero_is_a_note_off_and_released_notes_lose_member_messages() {
        let mut rx = MpeReceiver::new(MpeZones::LOWER_15);
        run(&mut rx, &[[0x91, 60, 100]]);
        let out = run(&mut rx, &[[0x91, 60, 0]]);
        assert_eq!(out, vec![MpeEvent::NoteOff { note: NoteRef::midi(1, 60), velocity: 64 }]);
        assert!(!rx.is_active(1, 60));
        assert!(run(&mut rx, &[[0xE1, 0, 0x50]]).is_empty());
    }

    #[test]
    fn a_channel_in_no_zone_is_plain_midi() {
        let mut rx = MpeReceiver::new(MpeZones::new(3, 0));
        let out = run(&mut rx, &[[0x99, 60, 100], [0xE9, 0, 0x7F], [0xD9, 50, 0], [0x89, 60, 10]]);
        assert_eq!(
            out,
            vec![
                MpeEvent::NoteOn { note: NoteRef::midi(9, 60), velocity: 100 },
                MpeEvent::Midi([0xE9, 0, 0x7F]),
                MpeEvent::Midi([0xD9, 50, 0]),
                MpeEvent::NoteOff { note: NoteRef::midi(9, 60), velocity: 10 },
            ]
        );
        // No zone at all: everything is plain.
        let mut rx = MpeReceiver::new(MpeZones::NONE);
        let out = run(&mut rx, &[[0x91, 60, 100], [0xE1, 0, 0x7F]]);
        assert_eq!(out[1], MpeEvent::Midi([0xE1, 0, 0x7F]));
    }

    #[test]
    fn hostile_bytes_never_panic() {
        let mut rx = MpeReceiver::default();
        let mut state = 0x1234_5678u32;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let b = state.to_le_bytes();
            rx.receive([b[0], b[1], b[2]], |_| {});
        }
        assert_eq!(bend_unit(0), -1.0);
        assert_eq!(bend_unit(16383), 1.0);
        assert_eq!(bend_unit(8192), 0.0);
        assert_eq!(bend_unit(u16::MAX), 1.0);
    }
}
