//! Identity pins through the shell ABI (ABI 8): pinned VST3 class ids and Audio
//! Unit codes, the parameter ids a VST3 and an Audio Unit host store, the AAX
//! parameter ids, and the state by parameter id. Its own test binary: the shell
//! caches one plugin's descriptor per binary.

use openplugin_core::identity::{check_pins, AuIds, Vst3Ids};
use openplugin_core::{decode_session, Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_ffi::shell::{self, OpParamDesc, OpShellInfo};

/// Three parameters with different numbers per format, as a port has when the
/// old plugin's formats did not agree; `c` is left to its position in VST3.
static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("a", "A", 0.0, 1.0, 0.25, Unit::None).with_host_id(1000).with_aax_id("AaxA"),
    ParamInfo::new("b", "B", 0.0, 1.0, 0.5, Unit::None).with_vst3_id(0x7FFF_FFFF).with_au_id(5),
    ParamInfo::new("c", "C", 0.0, 1.0, 0.75, Unit::None).with_au_id(0xFFFF_FFFE).with_clap_id(9),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.ids",
    name: "Ids",
    vendor: "Test",
    url: "",
    email: "",
    version: "1.0.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

const PROCESSOR: &str = "00112233445566778899AABBCCDDEEFF";

#[derive(Default)]
struct Ids {
    values: [f32; 3],
}

impl Plugin for Ids {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(index) = PARAMS.iter().position(|p| p.id == id) else { return false };
        self.values[index] = PARAMS[index].clamp(value);
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|index| self.values[index])
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    // A single-component source: only the processor id is pinned.
    fn vst3_ids(&self) -> Option<Vst3Ids> {
        Vst3Ids::processor_from_hex(PROCESSOR)
    }
    fn au_ids(&self) -> Option<AuIds> {
        Some(AuIds { subtype: *b"IdTs", manufacturer: *b"OpTs" })
    }
}

fn hex(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn aax_id(index: u32, cap: usize) -> (usize, String) {
    let mut buffer = vec![0x55u8; cap.max(1)];
    let length = unsafe { shell::param_aax_id::<Ids>(index, if cap == 0 { std::ptr::null_mut() } else { buffer.as_mut_ptr() }, cap) };
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(0);
    (length, String::from_utf8(buffer[..end].to_vec()).unwrap())
}

#[test]
fn the_pins_of_the_test_plugin_are_usable() {
    assert_eq!(check_pins(&Ids::default()), []);
}

#[test]
fn a_pinned_processor_id_replaces_the_derived_one_and_nothing_else() {
    let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(unsafe { shell::write_info::<Ids>(&mut info) }, 1);
    assert_eq!(info.abi, 8);
    assert_eq!(hex(&info.processor_cid), PROCESSOR);
    // The controller id and the development AAX codes are still the ones derived from
    // the plugin id and vendor: the pin does not seed them. (Computed apart from this
    // library, from the derivation in docs/VERSIONING.md.)
    assert_eq!(hex(&info.controller_cid), "50AE5FFBE4FF063C128A80A81FFBEA9E");
    assert_eq!(
        (info.manufacturer_id, info.product_id, info.plugin_id_mono, info.plugin_id_stereo),
        (0x3BA5_E82E, 0xBAAE_7832, 0x3531_E7F9, 0xFB7F_AE70)
    );
}

#[test]
fn each_format_gets_its_own_parameter_ids() {
    let ids = |format| (0..3).map(|index| shell::param_host_id::<Ids>(format, index)).collect::<Vec<_>>();
    assert_eq!(ids(shell::HOST_VST3), [1000, 0x7FFF_FFFF, 2]);
    assert_eq!(ids(shell::HOST_AU), [1000, 5, 0xFFFF_FFFE]);
    // An unknown index or format has no id.
    assert_eq!(shell::param_host_id::<Ids>(shell::HOST_VST3, 3), shell::NO_HOST_ID);
    assert_eq!(shell::param_host_id::<Ids>(shell::HOST_AU, u32::MAX), shell::NO_HOST_ID);
    for format in [0, 3, u32::MAX] {
        assert_eq!(shell::param_host_id::<Ids>(format, 0), shell::NO_HOST_ID);
    }
    // The descriptor still names parameters by their string id; the ABI still takes indices.
    let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
    assert_eq!(unsafe { shell::write_param::<Ids>(1, &mut desc) }, 1);
    assert!(desc.id.starts_with(b"b\0") && desc.name.starts_with(b"B\0"));
}

#[test]
fn the_aax_id_is_the_pin_or_the_string_id() {
    assert_eq!(aax_id(0, 64), (4, "AaxA".to_string()));
    assert_eq!(aax_id(1, 64), (1, "b".to_string()));
    assert_eq!(aax_id(2, 64), (1, "c".to_string()));
    // The length rules of the other text calls: the whole length is returned, the text is cut.
    assert_eq!(aax_id(0, 3), (4, "Aa".to_string()));
    assert_eq!(aax_id(0, 1), (4, String::new()));
    assert_eq!(unsafe { shell::param_aax_id::<Ids>(0, std::ptr::null_mut(), 0) }, 4);
    assert_eq!(aax_id(3, 64), (0, String::new()));
    assert_eq!(aax_id(u32::MAX, 64), (0, String::new()));
}

#[test]
fn the_audio_unit_codes_and_the_identity_bits() {
    assert_eq!(shell::au_code::<Ids>(shell::AU_SUBTYPE), u32::from_be_bytes(*b"IdTs"));
    assert_eq!(shell::au_code::<Ids>(shell::AU_MANUFACTURER), u32::from_be_bytes(*b"OpTs"));
    assert_eq!(shell::au_code::<Ids>(2), 0);
    assert_eq!(shell::au_code::<Ids>(u32::MAX), 0);
    assert_eq!(
        shell::identity::<Ids>(),
        shell::IDENTITY_KEYED_STATE
            | shell::IDENTITY_VST3_CLASS
            | shell::IDENTITY_AU_CODES
            | shell::IDENTITY_VST3_PARAMS
            | shell::IDENTITY_AU_PARAMS
            | shell::IDENTITY_AAX_PARAMS
    );
}

#[test]
fn the_state_blob_names_its_values() {
    let instance = shell::create::<Ids>(48_000.0, 64, 2);
    assert!(!instance.is_null());
    for (index, value) in [0.125f32, 0.375, 0.625].into_iter().enumerate() {
        assert_eq!(unsafe { shell::set_plain(instance, index as u32, value) }, 1);
    }
    let mut needed = 0u32;
    assert_eq!(unsafe { shell::save_state(instance, std::ptr::null_mut(), 0, &mut needed) }, shell::STATE_TOO_SMALL);
    let mut blob = vec![0u8; needed as usize];
    assert_eq!(unsafe { shell::save_state(instance, blob.as_mut_ptr(), needed, &mut needed) }, shell::STATE_OK);
    // An OPCH session whose plane is version 2: each value under its string id.
    assert_eq!(&blob[..4], b"OPCH");
    assert_eq!(&blob[10..18], b"OPST\x02\x00\x03\x00");
    assert_eq!(decode_session(&INFO, &blob).unwrap().values, vec![0.125, 0.375, 0.625]);
    // The same values in another order, as a build with a reordered table would have
    // written them, load onto the same parameters.
    let mut reordered = b"OPST\x02\x00\x03\x00".to_vec();
    for (id, value) in [("c", 0.5f32), ("a", 1.0), ("b", 0.0)] {
        reordered.extend_from_slice(&(id.len() as u16).to_le_bytes());
        reordered.extend_from_slice(id.as_bytes());
        reordered.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(unsafe { shell::load_state(instance, reordered.as_ptr(), reordered.len() as u32) }, 1);
    assert_eq!([0, 1, 2].map(|index| unsafe { shell::get_plain(instance, index) }), [1.0, 0.0, 0.5]);
    unsafe { shell::destroy(instance) };
}
