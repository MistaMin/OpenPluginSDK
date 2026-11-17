//! Pinned: the test plugin of the identity pinning proof (porting toolkit task 2).
//!
//! One small effect in four builds. Two of them pin every identifier a host
//! stores (`openplugin::core::identity`), as a port of a plugin from another
//! SDK does, and differ in their parameter tables:
//!
//! - [`PinnedA`]: `level`, `offset`, `invert`;
//! - [`PinnedB`]: `invert`, `drive` (new), `offset`, `level`.
//!
//! A project saved with one must reopen in the other: the same class ids, the
//! same parameter ids, state and automation landing on the same parameters.
//! [`PlainA`] and [`PlainB`] are the same two tables without a single pin, the
//! negative control: there a parameter's id is its position, so the same
//! project lands on the wrong parameters or is refused.
//!
//! The audio is `(in * level + offset) * (invert ? -1 : 1) * (1 + drive)` on
//! every channel, so each parameter is heard apart from the others and every
//! value used by the proofs is exact in `f32`.
//!
//! The fake hosts are `tests/session.rs` (CLAP) and the native identity proofs
//! (`native/vst3/identity_host.cpp`, `native/au/identity_host.cpp`,
//! `native/aax/host_identity.cpp`) over the static libraries of `builds/`.
//! Test builds only.

use openplugin::core::identity::{AuIds, Vst3Ids};
use openplugin::core::AaxIds;
use openplugin::prelude::*;

/// The ids a host of the "old" plugin stored: any numbers, as another SDK
/// derives them (here the parameter's name as four characters). The Audio
/// Unit of the old plugin numbered `level` differently, so that format has a
/// pin of its own.
pub const LEVEL_ID: u32 = 0x6C65_766C;
pub const LEVEL_AU_ID: u32 = 7;
pub const OFFSET_ID: u32 = 0x6F66_6673;
pub const INVERT_ID: u32 = 0x696E_7672;
pub const DRIVE_ID: u32 = 0x6472_6976;

/// The pinned plugin ids.
pub const CLAP_ID: &str = "com.example.legacy.pinned";
pub const VST3_PROCESSOR: &str = "A1B2C3D4E5F60718293A4B5C6D7E8F90";
pub const VST3_CONTROLLER: &str = "0F1E2D3C4B5A69788796A5B4C3D2E1F0";
pub const AU_IDS: AuIds = AuIds { subtype: *b"PnEx", manufacturer: *b"OpTs" };
pub const AAX_IDS: AaxIds = AaxIds { manufacturer: *b"OpTs", product: *b"PnEx", mono: *b"PnEM", stereo: *b"PnES" };

const LEVEL: ParamInfo = ParamInfo::new("level", "Level", 0.0, 2.0, 1.0, Unit::None);
const OFFSET: ParamInfo = ParamInfo::new("offset", "Offset", -1.0, 1.0, 0.0, Unit::None);
const INVERT: ParamInfo = ParamInfo::new("invert", "Invert", 0.0, 1.0, 0.0, Unit::Toggle);
const DRIVE: ParamInfo = ParamInfo::new("drive", "Drive", 0.0, 1.0, 0.0, Unit::None);

const PIN_LEVEL: ParamInfo = LEVEL.with_host_id(LEVEL_ID).with_au_id(LEVEL_AU_ID).with_aax_id("Level");
const PIN_OFFSET: ParamInfo = OFFSET.with_host_id(OFFSET_ID).with_aax_id("Offset");
const PIN_INVERT: ParamInfo = INVERT.with_host_id(INVERT_ID).with_aax_id("Invert");
const PIN_DRIVE: ParamInfo = DRIVE.with_host_id(DRIVE_ID).with_aax_id("Drive");

static PARAMS_PINNED_A: [ParamInfo; 3] = [PIN_LEVEL, PIN_OFFSET, PIN_INVERT];
static PARAMS_PINNED_B: [ParamInfo; 4] = [PIN_INVERT, PIN_DRIVE, PIN_OFFSET, PIN_LEVEL];
static PARAMS_PLAIN_A: [ParamInfo; 3] = [LEVEL, OFFSET, INVERT];
static PARAMS_PLAIN_B: [ParamInfo; 4] = [INVERT, DRIVE, OFFSET, LEVEL];

const fn info(id: &'static str, name: &'static str, params: &'static [ParamInfo]) -> PluginInfo {
    PluginInfo::new(id, name, "OpenPlugin proofs", "1.0.0", Category::Utility, params)
}

// The two pinned builds deliberately have different `PluginInfo::id`s: every
// id a host stores comes from a pin, so the derived ones are never used.
static INFO_PINNED_A: PluginInfo = info("com.opengrid.example.pinned", "Pinned", &PARAMS_PINNED_A);
static INFO_PINNED_B: PluginInfo = info("com.opengrid.example.pinned.rewrite", "Pinned", &PARAMS_PINNED_B);
static INFO_PLAIN_A: PluginInfo = info("com.opengrid.example.plain", "Plain", &PARAMS_PLAIN_A);
static INFO_PLAIN_B: PluginInfo = info("com.opengrid.example.plain", "Plain", &PARAMS_PLAIN_B);

/// The effect. `REORDERED` picks the second parameter table, `PINS` the pinned ids.
#[derive(Debug, Clone)]
pub struct Pinned<const REORDERED: bool, const PINS: bool> {
    level: f32,
    offset: f32,
    invert: bool,
    drive: f32,
}

pub type PinnedA = Pinned<false, true>;
pub type PinnedB = Pinned<true, true>;
pub type PlainA = Pinned<false, false>;
pub type PlainB = Pinned<true, false>;

impl<const REORDERED: bool, const PINS: bool> Default for Pinned<REORDERED, PINS> {
    fn default() -> Self {
        Self { level: 1.0, offset: 0.0, invert: false, drive: 0.0 }
    }
}

impl<const REORDERED: bool, const PINS: bool> Pinned<REORDERED, PINS> {
    /// What one input sample becomes.
    pub fn shape(&self, sample: f32) -> f32 {
        let sign = if self.invert { -1.0 } else { 1.0 };
        (sample * self.level + self.offset) * sign * (1.0 + self.drive)
    }
}

impl<const REORDERED: bool, const PINS: bool> Plugin for Pinned<REORDERED, PINS> {
    fn info(&self) -> &'static PluginInfo {
        match (REORDERED, PINS) {
            (false, true) => &INFO_PINNED_A,
            (true, true) => &INFO_PINNED_B,
            (false, false) => &INFO_PLAIN_A,
            (true, false) => &INFO_PLAIN_B,
        }
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        // `drive` exists in the second table only.
        match id {
            "level" => self.level = LEVEL.clamp(value),
            "offset" => self.offset = OFFSET.clamp(value),
            "invert" => self.invert = INVERT.clamp(value) > 0.5,
            "drive" if REORDERED => self.drive = DRIVE.clamp(value),
            _ => return false,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "level" => Some(self.level),
            "offset" => Some(self.offset),
            "invert" => Some(f32::from(u8::from(self.invert))),
            "drive" if REORDERED => Some(self.drive),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            for sample in channel.iter_mut().take(frames) {
                *sample = self.shape(*sample);
            }
        }
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    fn state_is_parameters_only(&self) -> bool {
        true
    }

    fn clap_id(&self) -> Option<&'static str> {
        PINS.then_some(CLAP_ID)
    }

    fn vst3_ids(&self) -> Option<Vst3Ids> {
        if PINS { Vst3Ids::from_hex(VST3_PROCESSOR, VST3_CONTROLLER) } else { None }
    }

    fn au_ids(&self) -> Option<AuIds> {
        PINS.then_some(AU_IDS)
    }

    fn aax_ids(&self) -> Option<AaxIds> {
        PINS.then_some(AAX_IDS)
    }
}
