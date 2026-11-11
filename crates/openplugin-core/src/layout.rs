//! Speaker layouts a plugin can advertise.
//!
//! The default is one stereo main bus and no sidechain. Surround and ambisonic
//! layouts are opt-in. Channel order for surround follows the CLAP surround ids.
//! Ambisonic buses are ACN, SN3D. Nothing here allocates.

/// Largest bus the process scratch will size. Seventh-order ambisonics is 64.
pub const MAX_AUDIO_CHANNELS: usize = 64;

/// CLAP `CLAP_SURROUND_*` channel ids.
pub const SURROUND_FL: u8 = 0;
pub const SURROUND_FR: u8 = 1;
pub const SURROUND_FC: u8 = 2;
pub const SURROUND_LFE: u8 = 3;
pub const SURROUND_BL: u8 = 4;
pub const SURROUND_BR: u8 = 5;
pub const SURROUND_FLC: u8 = 6;
pub const SURROUND_FRC: u8 = 7;
pub const SURROUND_BC: u8 = 8;
pub const SURROUND_SL: u8 = 9;
pub const SURROUND_SR: u8 = 10;
pub const SURROUND_TC: u8 = 11;
pub const SURROUND_TFL: u8 = 12;
pub const SURROUND_TFC: u8 = 13;
pub const SURROUND_TFR: u8 = 14;
pub const SURROUND_TBL: u8 = 15;
pub const SURROUND_TBC: u8 = 16;
pub const SURROUND_TBR: u8 = 17;

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

/// How a host should label one bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortKind {
    Empty,
    Mono,
    Stereo,
    Surround,
    Ambisonic,
    Discrete,
}

/// One bus. `Ambisonic(order)` uses `(order + 1)²` channels, orders 1 through 7.
/// `Discrete(n)` is an unnamed bus of `n` channels, from 1 to [`MAX_AUDIO_CHANNELS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpeakerLayout {
    Absent,
    Mono,
    Stereo,
    Lcr,
    Quad,
    Surround50,
    Surround51,
    Surround61,
    Surround71,
    Surround714,
    Ambisonic(u8),
    Discrete(u16),
}

impl SpeakerLayout {
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

    pub fn surround_map(self) -> Option<&'static [u8]> {
        match self {
            Self::Lcr => Some(&MAP_LCR),
            Self::Quad => Some(&MAP_QUAD),
            Self::Surround50 => Some(&MAP_50),
            Self::Surround51 => Some(&MAP_51),
            Self::Surround61 => Some(&MAP_61),
            Self::Surround71 => Some(&MAP_71),
            Self::Surround714 => Some(&MAP_714),
            _ => None,
        }
    }

    pub fn surround_mask(self) -> Option<u64> {
        let map = self.surround_map()?;
        let mut mask = 0u64;
        for id in map {
            mask |= 1u64 << id;
        }
        Some(mask)
    }

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IoConfig {
    pub input: SpeakerLayout,
    pub output: SpeakerLayout,
    pub sidechain: Option<SpeakerLayout>,
}

/// The layout a plugin uses when it does not override [`crate::Plugin::supported_layouts`].
pub static STEREO_LAYOUTS: &[IoConfig] = &[IoConfig::STEREO];

impl IoConfig {
    pub const STEREO: Self = Self {
        input: SpeakerLayout::Stereo,
        output: SpeakerLayout::Stereo,
        sidechain: None,
    };

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

    pub const fn input_channels(self) -> usize {
        match self.input.channels() {
            Some(n) => n as usize,
            None => 0,
        }
    }

    pub const fn output_channels(self) -> usize {
        match self.output.channels() {
            Some(n) => n as usize,
            None => 0,
        }
    }

    pub const fn sidechain_channels(self) -> usize {
        match self.sidechain {
            Some(layout) => match layout.channels() {
                Some(n) => n as usize,
                None => 0,
            },
            None => 0,
        }
    }

    /// Main-thread name. Not for the audio thread.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    /// The plugin reports zero samples of tail.
    None,
    Finite(u32),
    /// The plugin does not know. Hosts that treat a missing property as
    /// "unsupported" can omit the tail extension.
    Unknown,
    Infinite,
}

impl Tail {
    /// CLAP sample count. `Unknown` has no count. Infinite is `u32::MAX`,
    /// which CLAP treats as infinite because it is at least `i32::MAX`.
    pub const fn clap_samples(self) -> Option<u32> {
        match self {
            Self::None => Some(0),
            Self::Finite(n) => Some(n),
            Self::Unknown => None,
            Self::Infinite => Some(u32::MAX),
        }
    }
}

/// Which note ports a plugin wants. The CLAP bridge still defaults to one input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotePorts {
    pub input: bool,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub struct ParamFlags(u32);

impl ParamFlags {
    pub const AUTOMATABLE: Self = Self(1 << 0);
    pub const STEPPED: Self = Self(1 << 1);
    pub const HIDDEN: Self = Self(1 << 2);
    pub const READ_ONLY: Self = Self(1 << 3);
    pub const BYPASS: Self = Self(1 << 4);
    pub const MOMENTARY: Self = Self(1 << 5);
    pub const PRESET_EXCLUDE: Self = Self(1 << 6);
    /// Changes latency or does work the audio thread cannot finish. Not automatable.
    pub const NOT_REALTIME: Self = Self(1 << 7);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// Per-channel peaks for buses wider than stereo. Only the first 16 channels
/// are stored. `channels` is the full bus width.
pub const BUS_METER_CHANNELS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusMeters {
    pub channels: u8,
    pub peak_db: [f32; BUS_METER_CHANNELS],
    pub reduction_db: [f32; BUS_METER_CHANNELS],
}

impl BusMeters {
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
