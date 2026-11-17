//! Role Tags: the test plugin of the channel order proof (0.6 task 8).
//!
//! It declares every speaker layout the SDK has, mono through 9.1.6, with the
//! same layout on input and output. For the selected layout it looks up the
//! role of every channel with [`SpeakerLayout::role_at`], as plugin code should,
//! and never assumes an index:
//!
//! - each input channel must hold [`in_tag`] of the role the plugin believes the
//!   channel has, on every sample;
//! - each output channel then carries [`out_tag`] of that role, or, on a sample
//!   whose input was not the expected tag, [`MISMATCH`] plus the input sample, so
//!   the host can tell which tag arrived instead.
//!
//! A host that writes the tag of each speaker into its own channel (in its own
//! channel order) and finds the output tag of the same speaker on the same
//! channel has proven that every role arrived at the right index, in and out.
//! The fake hosts are `tests/formats.rs` (CLAP, LV2) and the native role proofs
//! (`native/vst3/roles_host.cpp`, `native/au/roles_host.cpp`,
//! `native/aax/host_roles.cpp`), which share the tag values through
//! `native/include/role_tags.h`.
//!
//! Test builds only. Exported as CLAP, LV2 and the shell ABI.

use openplugin::core::layout::SURROUND_ROLE_COUNT;
use openplugin::prelude::*;

/// The code of a channel with no speaker role: the one channel of a mono bus.
pub const MONO_CODE: u8 = SURROUND_ROLE_COUNT;

/// Added to the input sample on an output channel whose input was wrong.
pub const MISMATCH: f32 = 2.0;

/// The level a host writes on an input channel whose speaker has `code` (a role
/// id, or [`MONO_CODE`]). Multiples of 1/32, so every shell carries them exactly.
pub fn in_tag(code: u8) -> f32 {
    (f32::from(code) + 1.0) / 32.0
}

/// The level the plugin writes on the output channel it believes has `code`.
pub fn out_tag(code: u8) -> f32 {
    -in_tag(code)
}

/// The code of channel `index` of `layout`: its role, or [`MONO_CODE`].
pub fn code_at(layout: SpeakerLayout, index: usize) -> u8 {
    layout.role_at(index).unwrap_or(MONO_CODE)
}

/// Every speaker layout of the SDK, in and out alike.
pub static LAYOUTS: [IoConfig; 13] = [
    same(SpeakerLayout::Mono),
    same(SpeakerLayout::Stereo),
    same(SpeakerLayout::Lcr),
    same(SpeakerLayout::Quad),
    same(SpeakerLayout::Surround50),
    same(SpeakerLayout::Surround51),
    same(SpeakerLayout::Surround61),
    same(SpeakerLayout::Surround71),
    same(SpeakerLayout::Surround714),
    same(SpeakerLayout::Surround512),
    same(SpeakerLayout::Surround514),
    same(SpeakerLayout::Surround712),
    same(SpeakerLayout::Surround916),
];

const fn same(layout: SpeakerLayout) -> IoConfig {
    IoConfig { input: layout, output: layout, sidechain: None }
}

// One parameter: the VST3 shell refuses a library without any.
static PARAMS: [ParamInfo; 1] = [ParamInfo::new("spare", "Spare", 0.0, 1.0, 0.0, Unit::Toggle)];

pub static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.role-tags",
    "Role Tags",
    "OpenGrid",
    "1.0.0",
    Category::Utility,
    &PARAMS,
)
.with_url("https://github.com/MistaMin/OpenPluginSDK");

pub struct RoleTags {
    layout: IoConfig,
    spare: f32,
}

impl Default for RoleTags {
    fn default() -> Self {
        Self { layout: IoConfig::STEREO, spare: 0.0 }
    }
}

impl RoleTags {
    /// The layout the host selected.
    pub fn layout(&self) -> IoConfig {
        self.layout
    }
}

impl Plugin for RoleTags {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _: f32, _: usize, _: usize) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id != "spare" {
            return false;
        }
        self.spare = PARAMS[0].clamp(value);
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        (id == "spare").then_some(self.spare)
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        &LAYOUTS
    }

    fn set_layout(&mut self, layout: IoConfig) -> bool {
        if !LAYOUTS.contains(&layout) {
            return false;
        }
        self.layout = layout;
        true
    }

    fn prepare_layout(&mut self, _: f32, _: usize, layout: IoConfig) {
        if LAYOUTS.contains(&layout) {
            self.layout = layout;
        }
    }

    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let channels = self.layout.main_channels().min(io.len());
        for (index, channel) in io.iter_mut().enumerate().take(channels) {
            let expected = in_tag(code_at(self.layout.input, index));
            let tag = out_tag(code_at(self.layout.output, index));
            for sample in &mut channel[..frames] {
                *sample = if (*sample - expected).abs() < 1e-6 { tag } else { MISMATCH + *sample };
            }
        }
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    fn aax_ids(&self) -> Option<openplugin::core::AaxIds> {
        Some(openplugin::core::AaxIds { manufacturer: *b"OpTs", product: *b"RlTg", mono: *b"RlTM", stereo: *b"RlTS" })
    }

    fn lv2_uri(&self) -> Option<&'static str> {
        Some("https://github.com/MistaMin/OpenPluginSDK/examples/role-tags")
    }
}

openplugin::targets::export_clap!(RoleTags);
openplugin::targets::export_lv2!(RoleTags);
openplugin::targets::export_shell!(RoleTags);
