//! Pins a format cannot carry are ignored as a whole for that format, and the
//! shell answers with the derived identifiers, as it does for a plugin without
//! pins. `check_pins` names each one. Its own test binary: the shell caches one
//! plugin's descriptor per binary.

use openplugin_core::identity::{check_pins, AuIds, PinError, PinFormat, Vst3Ids, VST3_BYPASS_PARAM_ID};
use openplugin_core::{AaxIds, Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_host::shell::{self, OpShellInfo};

static PARAMS: [ParamInfo; 3] = [
    // The VST3 shell's own bypass id; the same Audio Unit id as the next parameter.
    ParamInfo::new("a", "A", 0.0, 1.0, 0.0, Unit::None).with_vst3_id(VST3_BYPASS_PARAM_ID).with_au_id(4),
    ParamInfo::new("b", "B", 0.0, 1.0, 0.0, Unit::None).with_au_id(4),
    // An AAX id that is too long.
    ParamInfo::new("c", "C", 0.0, 1.0, 0.0, Unit::None).with_aax_id("an-id-of-more-than-thirty-one-characters"),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.ids",
    "Bad Ids",
    "Test",
    "1.0.0",
    Category::Utility,
    &PARAMS,
);

#[derive(Default)]
struct BadIds;

impl Plugin for BadIds {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, _: f32) -> bool {
        PARAMS.iter().any(|p| p.id == id)
    }
    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().any(|p| p.id == id).then_some(0.0)
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
    fn clap_id(&self) -> Option<&'static str> {
        Some("")
    }
    fn vst3_ids(&self) -> Option<Vst3Ids> {
        Some(Vst3Ids { processor: [7; 16], controller: Some([7; 16]) })
    }
    fn au_ids(&self) -> Option<AuIds> {
        Some(AuIds { subtype: [0, 1, 2, 3], manufacturer: *b"OpTs" })
    }
    fn aax_ids(&self) -> Option<AaxIds> {
        Some(AaxIds { manufacturer: *b"OpTs", product: *b"BdId", mono: *b"BdIM", stereo: *b"BdIM" })
    }
}

#[test]
fn check_pins_names_every_unusable_pin() {
    assert_eq!(
        check_pins(&BadIds),
        [
            PinError::Reserved { format: PinFormat::Vst3, index: 0, id: VST3_BYPASS_PARAM_ID },
            PinError::Duplicate { format: PinFormat::Au, first: 0, second: 1, id: 4 },
            PinError::AaxInvalid { index: 2 },
            PinError::ClapId,
            PinError::Vst3Class,
            PinError::AuCode,
            PinError::AaxCode,
        ]
    );
}

#[test]
fn the_shell_answers_with_the_derived_identifiers() {
    let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(unsafe { shell::write_info::<BadIds>(&mut info) }, 1);
    // The ids derived from "com.opengrid.test.ids" and the vendor "Test".
    let hex = |bytes: &[u8; 16]| bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
    assert_eq!(hex(&info.processor_cid), "D9E711154263CF6DC16772469D72CC99");
    assert_eq!(hex(&info.controller_cid), "50AE5FFBE4FF063C128A80A81FFBEA9E");
    assert_eq!(
        (info.manufacturer_id, info.product_id, info.plugin_id_mono, info.plugin_id_stereo),
        (0x3BA5_E82E, 0xBAAE_7832, 0x3531_E7F9, 0xFB7F_AE70)
    );
    for format in [shell::HOST_VST3, shell::HOST_AU] {
        assert_eq!([0, 1, 2].map(|index| shell::param_host_id::<BadIds>(format, index)), [0, 1, 2]);
    }
    let mut buffer = [0u8; 64];
    assert_eq!(unsafe { shell::param_aax_id::<BadIds>(2, buffer.as_mut_ptr(), 64) }, 1);
    assert_eq!(&buffer[..2], b"c\0");
    assert_eq!(shell::au_code::<BadIds>(shell::AU_SUBTYPE), 0);
    // The table still has pins, so the state is by parameter id; nothing else is pinned.
    assert_eq!(shell::identity::<BadIds>(), shell::IDENTITY_KEYED_STATE);
}
