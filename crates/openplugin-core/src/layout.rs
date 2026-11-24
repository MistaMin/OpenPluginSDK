//! Speaker layouts a plugin can advertise.
//!
//! The default is one stereo main bus and no sidechain. Surround and ambisonic
//! layouts are opt-in. Channel order for surround follows the CLAP surround ids,
//! extended by four SDK-only ids for the Atmos beds (see [`SURROUND_TSL`]).
//! Ambisonic buses are ACN, SN3D. Nothing here allocates.

/// Largest bus the process scratch will size. Seventh-order ambisonics is 64.
pub const MAX_AUDIO_CHANNELS: usize = 64;

// The CLAP `CLAP_SURROUND_*` channel ids, each the same number as in CLAP.
/// Front left (`CLAP_SURROUND_FL`).
pub const SURROUND_FL: u8 = 0;
/// Front right (`CLAP_SURROUND_FR`).
pub const SURROUND_FR: u8 = 1;
/// Front centre (`CLAP_SURROUND_FC`).
pub const SURROUND_FC: u8 = 2;
/// Low-frequency effects (`CLAP_SURROUND_LFE`).
pub const SURROUND_LFE: u8 = 3;
/// Back left (`CLAP_SURROUND_BL`).
pub const SURROUND_BL: u8 = 4;
/// Back right (`CLAP_SURROUND_BR`).
pub const SURROUND_BR: u8 = 5;
/// Front left of centre (`CLAP_SURROUND_FLC`).
pub const SURROUND_FLC: u8 = 6;
/// Front right of centre (`CLAP_SURROUND_FRC`).
pub const SURROUND_FRC: u8 = 7;
/// Back centre (`CLAP_SURROUND_BC`).
pub const SURROUND_BC: u8 = 8;
/// Side left (`CLAP_SURROUND_SL`).
pub const SURROUND_SL: u8 = 9;
/// Side right (`CLAP_SURROUND_SR`).
pub const SURROUND_SR: u8 = 10;
/// Top centre (`CLAP_SURROUND_TC`).
pub const SURROUND_TC: u8 = 11;
/// Top front left (`CLAP_SURROUND_TFL`).
pub const SURROUND_TFL: u8 = 12;
/// Top front centre (`CLAP_SURROUND_TFC`).
pub const SURROUND_TFC: u8 = 13;
/// Top front right (`CLAP_SURROUND_TFR`).
pub const SURROUND_TFR: u8 = 14;
/// Top back left (`CLAP_SURROUND_TBL`).
pub const SURROUND_TBL: u8 = 15;
/// Top back centre (`CLAP_SURROUND_TBC`).
pub const SURROUND_TBC: u8 = 16;
/// Top back right (`CLAP_SURROUND_TBR`).
pub const SURROUND_TBR: u8 = 17;

/// SDK-only role ids. CLAP has no ids for these channels, so a layout that
/// uses one cannot be expressed through the CLAP surround extension
/// ([`SpeakerLayout::clap_expressible`] is false). Top side left and right are
/// the overhead pair of the x.y.2 Atmos beds (Dolby "top middle", Ltm/Rtm);
/// wide left and right are the 9.1.6 front wides (Lw/Rw).
pub const SURROUND_TSL: u8 = 18;
/// Top side right (Dolby Rtm). SDK-only; see [`SURROUND_TSL`].
pub const SURROUND_TSR: u8 = 19;
/// Front left wide (Dolby Lw). SDK-only; see [`SURROUND_TSL`].
pub const SURROUND_FLW: u8 = 20;
/// Front right wide (Dolby Rw). SDK-only; see [`SURROUND_TSL`].
pub const SURROUND_FRW: u8 = 21;
/// Highest id CLAP defines (`CLAP_SURROUND_TBR`).
pub const CLAP_SURROUND_MAX: u8 = SURROUND_TBR;
/// One past the highest role id the SDK defines.
pub const SURROUND_ROLE_COUNT: u8 = 22;

/// The other half of a left/right pair, or `None` for an unpaired role.
pub const fn partner_role(role: u8) -> Option<u8> {
    Some(match role {
        SURROUND_FL => SURROUND_FR,
        SURROUND_FR => SURROUND_FL,
        SURROUND_BL => SURROUND_BR,
        SURROUND_BR => SURROUND_BL,
        SURROUND_FLC => SURROUND_FRC,
        SURROUND_FRC => SURROUND_FLC,
        SURROUND_SL => SURROUND_SR,
        SURROUND_SR => SURROUND_SL,
        SURROUND_TFL => SURROUND_TFR,
        SURROUND_TFR => SURROUND_TFL,
        SURROUND_TBL => SURROUND_TBR,
        SURROUND_TBR => SURROUND_TBL,
        SURROUND_TSL => SURROUND_TSR,
        SURROUND_TSR => SURROUND_TSL,
        SURROUND_FLW => SURROUND_FRW,
        SURROUND_FRW => SURROUND_FLW,
        _ => return None,
    })
}

/// True for the left member of a pair.
pub const fn is_left_role(role: u8) -> bool {
    matches!(
        role,
        SURROUND_FL
            | SURROUND_BL
            | SURROUND_FLC
            | SURROUND_SL
            | SURROUND_TFL
            | SURROUND_TBL
            | SURROUND_TSL
            | SURROUND_FLW
    )
}

const MAP_STEREO: [u8; 2] = [SURROUND_FL, SURROUND_FR];
const MAP_LCR: [u8; 3] = [SURROUND_FL, SURROUND_FR, SURROUND_FC];
const MAP_QUAD: [u8; 4] = [SURROUND_FL, SURROUND_FR, SURROUND_BL, SURROUND_BR];
const MAP_50: [u8; 5] = [SURROUND_FL, SURROUND_FR, SURROUND_FC, SURROUND_BL, SURROUND_BR];
const MAP_51: [u8; 6] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_BL,
    SURROUND_BR,
];
const MAP_61: [u8; 7] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_BL,
    SURROUND_BR,
    SURROUND_BC,
];
const MAP_71: [u8; 8] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_SL,
    SURROUND_SR,
    SURROUND_BL,
    SURROUND_BR,
];
const MAP_714: [u8; 12] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_SL,
    SURROUND_SR,
    SURROUND_BL,
    SURROUND_BR,
    SURROUND_TFL,
    SURROUND_TFR,
    SURROUND_TBL,
    SURROUND_TBR,
];
/// 5.1.2: 5.1, then the top side pair (Dolby L R C LFE Ls Rs Ltm Rtm).
const MAP_512: [u8; 8] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_BL,
    SURROUND_BR,
    SURROUND_TSL,
    SURROUND_TSR,
];
/// 5.1.4: 5.1, then front and back height pairs, as 7.1.4 does.
const MAP_514: [u8; 10] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_BL,
    SURROUND_BR,
    SURROUND_TFL,
    SURROUND_TFR,
    SURROUND_TBL,
    SURROUND_TBR,
];
/// 7.1.2: 7.1, then the top side pair.
const MAP_712: [u8; 10] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_SL,
    SURROUND_SR,
    SURROUND_BL,
    SURROUND_BR,
    SURROUND_TSL,
    SURROUND_TSR,
];
/// 9.1.6: 7.1, the front wides, then the front, top side and back heights.
const MAP_916: [u8; 16] = [
    SURROUND_FL,
    SURROUND_FR,
    SURROUND_FC,
    SURROUND_LFE,
    SURROUND_SL,
    SURROUND_SR,
    SURROUND_BL,
    SURROUND_BR,
    SURROUND_FLW,
    SURROUND_FRW,
    SURROUND_TFL,
    SURROUND_TFR,
    SURROUND_TSL,
    SURROUND_TSR,
    SURROUND_TBL,
    SURROUND_TBR,
];

/// How a host should label one bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PortKind {
    /// No channels ([`SpeakerLayout::Absent`]).
    Empty,
    /// One channel.
    Mono,
    /// Left and right.
    Stereo,
    /// A speaker layout with roles beyond stereo.
    Surround,
    /// An ambisonic bus (ACN, SN3D).
    Ambisonic,
    /// Unnamed channels.
    Discrete,
}

/// One bus. `Ambisonic(order)` uses `(order + 1)²` channels, orders 1 through 7.
/// `Discrete(n)` is an unnamed bus of `n` channels, from 1 to [`MAX_AUDIO_CHANNELS`].
///
/// ```
/// use openplugin_core::layout::{SpeakerLayout, SURROUND_LFE};
/// let bus = SpeakerLayout::Surround51;
/// assert_eq!(bus.channels(), Some(6));
/// assert_eq!(bus.index_of(SURROUND_LFE), Some(3));
/// assert_eq!(bus.pairs().collect::<Vec<_>>(), [(0, 1), (4, 5)]);
/// assert_eq!(SpeakerLayout::Ambisonic(8).channels(), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SpeakerLayout {
    /// No bus: 0 channels.
    Absent,
    /// One channel, no speaker role.
    Mono,
    /// FL FR.
    Stereo,
    /// FL FR FC.
    Lcr,
    /// FL FR BL BR.
    Quad,
    /// FL FR FC BL BR.
    Surround50,
    /// FL FR FC LFE BL BR.
    Surround51,
    /// FL FR FC LFE BL BR BC.
    Surround61,
    /// FL FR FC LFE SL SR BL BR.
    Surround71,
    /// 7.1 then TFL TFR TBL TBR, 12 channels.
    Surround714,
    /// 5.1.2 Atmos bed. Uses SDK-only roles: not offered through CLAP.
    Surround512,
    /// 5.1.4 Atmos bed.
    Surround514,
    /// 7.1.2 Atmos bed. Not offered through CLAP.
    Surround712,
    /// 9.1.6 Atmos bed, 16 channels. Not offered through CLAP.
    Surround916,
    /// Ambisonics of the given order (1..=7), `(order + 1)²` channels.
    Ambisonic(u8),
    /// `n` unnamed channels, 1..=[`MAX_AUDIO_CHANNELS`].
    Discrete(u16),
}

impl SpeakerLayout {
    /// Channel count; `None` for an ambisonic order outside 1..=7 or a
    /// discrete count outside 1..=[`MAX_AUDIO_CHANNELS`].
    pub const fn channels(self) -> Option<u16> {
        match self {
            Self::Absent => Some(0),
            Self::Mono => Some(1),
            Self::Stereo => Some(2),
            Self::Lcr => Some(3),
            Self::Quad => Some(4),
            Self::Surround50 => Some(5),
            Self::Surround51 => Some(6),
            Self::Surround61 => Some(7),
            Self::Surround71 => Some(8),
            Self::Surround714 => Some(12),
            Self::Surround512 => Some(8),
            Self::Surround514 | Self::Surround712 => Some(10),
            Self::Surround916 => Some(16),
            Self::Ambisonic(order) => {
                if order == 0 || order > 7 {
                    None
                } else {
                    let n = order as u16 + 1;
                    Some(n * n)
                }
            }
            Self::Discrete(n) => {
                if n >= 1 && n as usize <= MAX_AUDIO_CHANNELS {
                    Some(n)
                } else {
                    None
                }
            }
        }
    }

    /// How a host should label the bus.
    pub const fn kind(self) -> PortKind {
        match self {
            Self::Absent => PortKind::Empty,
            Self::Mono => PortKind::Mono,
            Self::Stereo => PortKind::Stereo,
            Self::Ambisonic(_) => PortKind::Ambisonic,
            Self::Discrete(_) => PortKind::Discrete,
            _ => PortKind::Surround,
        }
    }

    /// Role ids in channel order (the SDK's canonical order) for the speaker
    /// layouts, excluding mono and stereo.
    pub const fn surround_map(self) -> Option<&'static [u8]> {
        match self {
            Self::Lcr => Some(&MAP_LCR),
            Self::Quad => Some(&MAP_QUAD),
            Self::Surround50 => Some(&MAP_50),
            Self::Surround51 => Some(&MAP_51),
            Self::Surround61 => Some(&MAP_61),
            Self::Surround71 => Some(&MAP_71),
            Self::Surround714 => Some(&MAP_714),
            Self::Surround512 => Some(&MAP_512),
            Self::Surround514 => Some(&MAP_514),
            Self::Surround712 => Some(&MAP_712),
            Self::Surround916 => Some(&MAP_916),
            _ => None,
        }
    }

    /// Bit mask of the roles, bit `n` for role id `n`. For the layouts CLAP
    /// can express this is the `CLAP_SURROUND_*` channel mask; `None` for
    /// layouts that use an SDK-only role (see [`Self::clap_expressible`]) and
    /// for layouts without a speaker map.
    pub const fn surround_mask(self) -> Option<u64> {
        if !self.clap_expressible() {
            return None;
        }
        let Some(map) = self.surround_map() else { return None };
        let mut mask = 0u64;
        let mut i = 0;
        while i < map.len() {
            mask |= 1u64 << map[i];
            i += 1;
        }
        Some(mask)
    }

    /// True for a speaker layout whose roles are all CLAP surround ids.
    /// False for the Atmos beds that use [`SURROUND_TSL`] and friends, and
    /// for layouts with no speaker map.
    pub const fn clap_expressible(self) -> bool {
        let Some(map) = self.surround_map() else { return false };
        let mut i = 0;
        while i < map.len() {
            if map[i] > CLAP_SURROUND_MAX {
                return false;
            }
            i += 1;
        }
        true
    }

    /// [`Self::surround_map`] if CLAP can express it, else `None`. This is
    /// what a `clap_plugin_surround.get_channel_map` should return.
    pub const fn clap_map(self) -> Option<&'static [u8]> {
        if self.clap_expressible() { self.surround_map() } else { None }
    }

    /// Role of every channel in canonical order: the surround map, `FL FR`
    /// for stereo, and empty for mono (a mono channel has no speaker role),
    /// ambisonic, discrete and absent buses.
    pub const fn roles(self) -> &'static [u8] {
        match self {
            Self::Stereo => &MAP_STEREO,
            _ => match self.surround_map() {
                Some(map) => map,
                None => &[],
            },
        }
    }

    /// Channel index of `role`, or `None` if the layout has no such channel.
    pub const fn index_of(self, role: u8) -> Option<usize> {
        let roles = self.roles();
        let mut i = 0;
        while i < roles.len() {
            if roles[i] == role {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// Role of channel `index`, if it has one.
    pub const fn role_at(self, index: usize) -> Option<u8> {
        let roles = self.roles();
        if index < roles.len() { Some(roles[index]) } else { None }
    }

    /// True when channel `index` is the LFE.
    pub const fn is_lfe(self, index: usize) -> bool {
        matches!(self.role_at(index), Some(SURROUND_LFE))
    }

    /// Indices of every channel except the LFE, ascending. Works for every
    /// layout (mono, ambisonic and discrete buses have no LFE).
    pub fn non_lfe(self) -> impl Iterator<Item = usize> {
        let count = self.channels().unwrap_or(0) as usize;
        (0..count).filter(move |&i| !self.is_lfe(i))
    }

    /// Left/right partner pairs as `(left, right)` channel indices, in
    /// ascending left index. Only roles that have a partner in this layout.
    pub fn pairs(self) -> impl Iterator<Item = (usize, usize)> {
        self.roles().iter().enumerate().filter_map(move |(i, &role)| {
            if !is_left_role(role) {
                return None;
            }
            Some((i, self.index_of(partner_role(role)?)?))
        })
    }

    /// Channels that belong to no pair, ascending: the centre, back centre,
    /// top centre, the LFE, and every channel of a bus with no roles (mono,
    /// ambisonic, discrete). Filter with [`Self::is_lfe`] to leave the LFE out.
    pub fn unpaired(self) -> impl Iterator<Item = usize> {
        let count = self.channels().unwrap_or(0) as usize;
        (0..count).filter(move |&i| match self.role_at(i) {
            Some(role) => match partner_role(role) {
                Some(p) => self.index_of(p).is_none(),
                None => true,
            },
            None => true,
        })
    }

    /// Short English name (`"5.1"`, `"HOA3"`); `"HOA"` for an invalid order.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Absent => "None",
            Self::Mono => "Mono",
            Self::Stereo => "Stereo",
            Self::Lcr => "LCR",
            Self::Quad => "Quad",
            Self::Surround50 => "5.0",
            Self::Surround51 => "5.1",
            Self::Surround61 => "6.1",
            Self::Surround71 => "7.1",
            Self::Surround714 => "7.1.4",
            Self::Surround512 => "5.1.2",
            Self::Surround514 => "5.1.4",
            Self::Surround712 => "7.1.2",
            Self::Surround916 => "9.1.6",
            Self::Ambisonic(1) => "HOA1",
            Self::Ambisonic(2) => "HOA2",
            Self::Ambisonic(3) => "HOA3",
            Self::Ambisonic(4) => "HOA4",
            Self::Ambisonic(5) => "HOA5",
            Self::Ambisonic(6) => "HOA6",
            Self::Ambisonic(7) => "HOA7",
            Self::Ambisonic(_) => "HOA",
            Self::Discrete(_) => "Discrete",
        }
    }
}

/// Main input, main output, and an optional sidechain input.
///
/// The sidechain is a separate bus. It is not mixed into the main channels.
///
/// ```
/// use openplugin_core::{IoConfig, SpeakerLayout};
/// static LAYOUTS: [IoConfig; 2] = [
///     IoConfig::STEREO,
///     IoConfig { input: SpeakerLayout::Mono, output: SpeakerLayout::Stereo, sidechain: Some(SpeakerLayout::Stereo) },
/// ];
/// assert_eq!(LAYOUTS[1].main_channels(), 2);
/// assert_eq!(LAYOUTS[1].sidechain_channels(), 2);
/// assert_eq!(LAYOUTS[1].name(), "Mono to Stereo + Stereo sidechain");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IoConfig {
    /// Main input bus; [`SpeakerLayout::Absent`] for an instrument.
    pub input: SpeakerLayout,
    /// Main output bus.
    pub output: SpeakerLayout,
    /// Sidechain input bus, if any.
    pub sidechain: Option<SpeakerLayout>,
}

/// The layout a plugin uses when it does not override [`crate::Plugin::supported_layouts`].
pub static STEREO_LAYOUTS: &[IoConfig] = &[IoConfig::STEREO];

impl IoConfig {
    /// Stereo in, stereo out, no sidechain.
    pub const STEREO: Self = Self {
        input: SpeakerLayout::Stereo,
        output: SpeakerLayout::Stereo,
        sidechain: None,
    };

    /// The wider of the main input and output, in channels: the number of
    /// slices `process` gets. An invalid layout counts as 0.
    pub const fn main_channels(self) -> usize {
        let input = match self.input.channels() {
            Some(n) => n as usize,
            None => 0,
        };
        let output = match self.output.channels() {
            Some(n) => n as usize,
            None => 0,
        };
        if input > output { input } else { output }
    }

    /// Main input channels; 0 for an absent or invalid layout.
    pub const fn input_channels(self) -> usize {
        match self.input.channels() {
            Some(n) => n as usize,
            None => 0,
        }
    }

    /// Main output channels; 0 for an absent or invalid layout.
    pub const fn output_channels(self) -> usize {
        match self.output.channels() {
            Some(n) => n as usize,
            None => 0,
        }
    }

    /// Sidechain channels; 0 without a sidechain or for an invalid layout.
    pub const fn sidechain_channels(self) -> usize {
        match self.sidechain {
            Some(layout) => match layout.channels() {
                Some(n) => n as usize,
                None => 0,
            },
            None => 0,
        }
    }

    /// Name for a host menu, such as `"Mono to Stereo + Stereo sidechain"`.
    /// Allocates: main thread only.
    pub fn name(self) -> String {
        let mut text = if self.input == SpeakerLayout::Absent {
            format!("{} out", self.output.label())
        } else if self.input == self.output {
            self.output.label().to_string()
        } else {
            format!("{} to {}", self.input.label(), self.output.label())
        };
        if let Some(side) = self.sidechain {
            text.push_str(" + ");
            text.push_str(side.label());
            text.push_str(" sidechain");
        }
        text
    }
}

/// Decay after the input stops. This is not the delay-compensation latency.
///
/// ```
/// use openplugin_core::Tail;
/// let two_seconds = Tail::from_samples(96_000);
/// assert_eq!(two_seconds.to_clap_samples(), Some(96_000));
/// assert_eq!(Tail::from_samples(0), Tail::None);
/// assert_eq!(Tail::Unknown.to_clap_samples(), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tail {
    /// The plugin reports zero samples of tail.
    None,
    /// A tail of this many samples.
    Finite(u32),
    /// The plugin does not know. Hosts that treat a missing property as
    /// "unsupported" can omit the tail extension.
    Unknown,
    /// The output may never fall silent (an infinite reverb or a drone).
    Infinite,
}

impl Tail {
    /// A sample count as a tail: zero is `None`, anything else `Finite`.
    pub const fn from_samples(samples: usize) -> Self {
        match samples {
            0 => Self::None,
            n if n > u32::MAX as usize => Self::Finite(u32::MAX),
            n => Self::Finite(n as u32),
        }
    }

    /// CLAP sample count. `Unknown` has no count. Infinite is `u32::MAX`,
    /// which CLAP treats as infinite because it is at least `i32::MAX`.
    pub const fn to_clap_samples(self) -> Option<u32> {
        match self {
            Self::None => Some(0),
            Self::Finite(n) => Some(n),
            Self::Unknown => None,
            Self::Infinite => Some(u32::MAX),
        }
    }
}

/// Which note ports a plugin wants. The CLAP bridge still defaults to one input.
///
/// ```
/// use openplugin_core::NotePorts;
/// // An audio effect that reads no MIDI.
/// let ports = NotePorts { input: false, output: false };
/// assert!(NotePorts::default().input);
/// # let _ = ports;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotePorts {
    /// One note input port (notes and MIDI in).
    pub input: bool,
    /// One note output port (what [`crate::Plugin::pull_midi`] returns).
    pub output: bool,
}

impl Default for NotePorts {
    fn default() -> Self {
        Self {
            input: true,
            output: false,
        }
    }
}

/// Flags on a [`crate::ParamInfo`]. Existing parameters stay automatable.
/// The bit of each flag is part of the API ([`ParamFlags::bits`] is
/// serialised and crosses the shell ABI) and never changes: `AUTOMATABLE` 1,
/// `STEPPED` 2, `HIDDEN` 4, `READ_ONLY` 8, `BYPASS` 16, `MOMENTARY` 32,
/// `PRESET_EXCLUDE` 64, `NOT_REALTIME` 128, `MODULATABLE` 256,
/// `POLY_MODULATION` 256 + 512.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
///
/// ```
/// use openplugin_core::{ParamFlags, ParamInfo, Unit};
/// let listen = ParamInfo::new("listen", "Listen", 0.0, 1.0, 0.0, Unit::Toggle)
///     .with_flag(ParamFlags::MOMENTARY)
///     .with_flag(ParamFlags::PRESET_EXCLUDE);
/// assert!(listen.flags.contains(ParamFlags::AUTOMATABLE.union(ParamFlags::MOMENTARY)));
/// assert_eq!(listen.flags.bits(), 1 | 32 | 64);
/// ```
pub struct ParamFlags(u32);

impl ParamFlags {
    /// The host may record and play automation for it.
    pub const AUTOMATABLE: Self = Self(1 << 0);
    /// The host shows it as discrete (whole values).
    pub const STEPPED: Self = Self(1 << 1);
    /// Not shown in the host's generic parameter list.
    pub const HIDDEN: Self = Self(1 << 2);
    /// The plugin sets it (a meter, a readout); the host may not. Not automatable.
    pub const READ_ONLY: Self = Self(1 << 3);
    /// The plugin's bypass switch; a host may map its own bypass button to it.
    pub const BYPASS: Self = Self(1 << 4);
    /// Springs back to its default: reset by [`crate::release_momentary`].
    pub const MOMENTARY: Self = Self(1 << 5);
    /// Marks a parameter a preset should leave alone (an output trim, a
    /// listen switch). No SDK code reads it yet: presets and saved state
    /// still include the parameter.
    pub const PRESET_EXCLUDE: Self = Self(1 << 6);
    /// Changes latency or does work the audio thread cannot finish. Not automatable.
    pub const NOT_REALTIME: Self = Self(1 << 7);
    /// The host may modulate the parameter for the whole plugin (CLAP
    /// `CLAP_PARAM_IS_MODULATABLE`), delivered through
    /// [`crate::Plugin::param_modulation`] with [`crate::NoteRef::ALL`].
    pub const MODULATABLE: Self = Self(1 << 8);
    /// The host may modulate the parameter per note: by note id, key, channel
    /// and port (CLAP `CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID`, `_PER_KEY`,
    /// `_PER_CHANNEL`, `_PER_PORT`), and for the whole plugin. Implies
    /// [`Self::MODULATABLE`].
    pub const POLY_MODULATION: Self = Self((1 << 8) | (1 << 9));

    /// The flag word, as serialised and passed across the shell ABI.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every flag in `other` is set (true for an empty `other`).
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The flags of both.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The flags of `self` that are not in `other`.
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// Per-channel peaks for buses wider than stereo. Only the first 16 channels
/// are stored. `channels` is the full bus width.
pub const BUS_METER_CHANNELS: usize = 16;

/// Per-channel peak and gain reduction of a bus wider than stereo, returned
/// by [`crate::Plugin::bus_meters`] on the audio thread.
///
/// ```
/// use openplugin_core::BusMeters;
/// let mut meters = BusMeters::SILENT;
/// meters.channels = 6;
/// meters.peak_db[3] = -18.0; // the LFE of a 5.1 bus
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusMeters {
    /// The bus width; only the first [`BUS_METER_CHANNELS`] are stored. 0 for no meters.
    pub channels: u8,
    /// Peak level per channel, dBFS.
    pub peak_db: [f32; BUS_METER_CHANNELS],
    /// Gain reduction per channel in dB, positive for attenuation.
    pub reduction_db: [f32; BUS_METER_CHANNELS],
}

impl BusMeters {
    /// No channels, every peak at -100 dB and no reduction.
    pub const SILENT: Self = Self {
        channels: 0,
        peak_db: [-100.0; BUS_METER_CHANNELS],
        reduction_db: [0.0; BUS_METER_CHANNELS],
    };
}

impl Default for BusMeters {
    fn default() -> Self {
        Self::SILENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surround_and_ambisonic_counts_match_the_maps() {
        assert_eq!(SpeakerLayout::Surround51.channels(), Some(6));
        assert_eq!(SpeakerLayout::Surround51.surround_map().unwrap().len(), 6);
        assert_eq!(SpeakerLayout::Surround714.channels(), Some(12));
        assert_eq!(SpeakerLayout::Surround714.surround_map().unwrap()[8], SURROUND_TFL);
        assert_eq!(SpeakerLayout::Ambisonic(1).channels(), Some(4));
        assert_eq!(SpeakerLayout::Ambisonic(3).channels(), Some(16));
        assert_eq!(SpeakerLayout::Ambisonic(7).channels(), Some(64));
        assert_eq!(SpeakerLayout::Ambisonic(8).channels(), None);
        assert_eq!(SpeakerLayout::Discrete(64).channels(), Some(64));
        assert_eq!(SpeakerLayout::Discrete(65).channels(), None);
        assert_eq!(SpeakerLayout::Surround51.kind(), PortKind::Surround);
        assert_eq!(SpeakerLayout::Ambisonic(2).kind(), PortKind::Ambisonic);
    }

    const SPEAKER_LAYOUTS: [SpeakerLayout; 12] = [
        SpeakerLayout::Stereo,
        SpeakerLayout::Lcr,
        SpeakerLayout::Quad,
        SpeakerLayout::Surround50,
        SpeakerLayout::Surround51,
        SpeakerLayout::Surround61,
        SpeakerLayout::Surround71,
        SpeakerLayout::Surround714,
        SpeakerLayout::Surround512,
        SpeakerLayout::Surround514,
        SpeakerLayout::Surround712,
        SpeakerLayout::Surround916,
    ];

    #[test]
    fn roles_are_unique_and_index_of_round_trips() {
        for layout in SPEAKER_LAYOUTS {
            let roles = layout.roles();
            assert_eq!(roles.len(), layout.channels().unwrap() as usize, "{}", layout.label());
            for (i, &role) in roles.iter().enumerate() {
                assert!(role < SURROUND_ROLE_COUNT);
                assert_eq!(layout.index_of(role), Some(i));
                assert_eq!(layout.role_at(i), Some(role));
                assert_eq!(roles.iter().filter(|&&r| r == role).count(), 1, "{}", layout.label());
            }
            assert_eq!(layout.index_of(SURROUND_ROLE_COUNT), None);
        }
        assert!(SpeakerLayout::Mono.roles().is_empty());
        assert_eq!(SpeakerLayout::Surround51.index_of(SURROUND_LFE), Some(3));
        assert_eq!(SpeakerLayout::Surround714.index_of(SURROUND_TBR), Some(11));
        assert_eq!(SpeakerLayout::Surround916.index_of(SURROUND_TBR), Some(15));
    }

    #[test]
    fn pairs_and_unpaired_cover_every_non_lfe_channel_once() {
        for layout in SPEAKER_LAYOUTS {
            let mut seen = [0u8; 16];
            for (l, r) in layout.pairs() {
                assert!(is_left_role(layout.role_at(l).unwrap()));
                assert_eq!(partner_role(layout.role_at(l).unwrap()), layout.role_at(r));
                seen[l] += 1;
                seen[r] += 1;
            }
            for i in layout.unpaired().filter(|&i| !layout.is_lfe(i)) {
                seen[i] += 1;
            }
            for i in layout.non_lfe() {
                assert_eq!(seen[i], 1, "{} channel {i}", layout.label());
            }
            let lfe = (0..layout.channels().unwrap() as usize).filter(|&i| layout.is_lfe(i)).count();
            assert_eq!(layout.non_lfe().count() + lfe, layout.channels().unwrap() as usize);
            assert_eq!(layout.index_of(SURROUND_LFE).is_some(), lfe == 1);
        }
        let pairs: Vec<_> = SpeakerLayout::Surround51.pairs().collect();
        assert_eq!(pairs, [(0, 1), (4, 5)]);
        let un: Vec<_> = SpeakerLayout::Surround51.unpaired().collect();
        assert_eq!(un, [2, 3]);
        assert_eq!(SpeakerLayout::Surround916.pairs().count(), 7);
        assert_eq!(SpeakerLayout::Mono.unpaired().collect::<Vec<_>>(), [0]);
        assert_eq!(SpeakerLayout::Ambisonic(1).non_lfe().count(), 4);
    }

    #[test]
    fn atmos_beds() {
        for (layout, n) in [
            (SpeakerLayout::Surround512, 8),
            (SpeakerLayout::Surround514, 10),
            (SpeakerLayout::Surround712, 10),
            (SpeakerLayout::Surround916, 16),
        ] {
            assert_eq!(layout.channels(), Some(n));
            assert_eq!(layout.kind(), PortKind::Surround);
            assert_eq!(layout.surround_map().unwrap().len(), n as usize);
        }
        assert_eq!(SpeakerLayout::Surround512.label(), "5.1.2");
        assert_eq!(SpeakerLayout::Surround916.label(), "9.1.6");
        // CLAP can express 5.1.4 but not the beds with top side or wide channels.
        assert!(SpeakerLayout::Surround514.clap_expressible());
        assert!(SpeakerLayout::Surround514.surround_mask().is_some());
        for layout in [SpeakerLayout::Surround512, SpeakerLayout::Surround712, SpeakerLayout::Surround916] {
            assert!(!layout.clap_expressible());
            assert_eq!(layout.surround_mask(), None);
            assert_eq!(layout.clap_map(), None);
            assert!(layout.surround_map().is_some());
        }
        assert!(SpeakerLayout::Surround714.clap_expressible());
        assert_eq!(SpeakerLayout::Surround712.index_of(SURROUND_TSL), Some(8));
        assert_eq!(SpeakerLayout::Surround916.index_of(SURROUND_FLW), Some(8));
    }

    #[test]
    fn five_one_mask_is_the_front_lfe_and_rear_pair() {
        let mask = SpeakerLayout::Surround51.surround_mask().unwrap();
        for id in [SURROUND_FL, SURROUND_FR, SURROUND_FC, SURROUND_LFE, SURROUND_BL, SURROUND_BR] {
            assert_ne!(mask & (1u64 << id), 0);
        }
        assert_eq!(mask & (1u64 << SURROUND_SL), 0);
    }

    #[test]
    fn sidechain_is_not_part_of_the_main_count() {
        let config = IoConfig {
            input: SpeakerLayout::Surround51,
            output: SpeakerLayout::Surround51,
            sidechain: Some(SpeakerLayout::Stereo),
        };
        assert_eq!(config.main_channels(), 6);
        assert_eq!(config.sidechain_channels(), 2);
        assert!(config.name().contains("5.1"));
        assert!(config.name().contains("sidechain"));
    }
}
