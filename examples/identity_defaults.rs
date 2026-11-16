// Shared by tests/identity.rs of the examples that export the shell ABI
// (`include!`): an example without identity pins must show every host the
// identifiers it showed before pins existed, bit for bit. The expected values
// are literals, computed apart from the SDK from the derivation written down in
// docs/VERSIONING.md (and, for the editor example, the class ids
// `openplugin-port`'s scan test has always pinned).

use std::ffi::CStr;

use openplugin::core::identity::{check_pins, params_pinned, state_is_keyed};
use openplugin::core::{encode_params, Plugin};
use openplugin::ffi::shell::{self, OpParamDesc, OpShellInfo};
use openplugin::targets::clap_abi::{clap_param_info, clap_plugin_params};
use openplugin::targets::ClapAdapter;

/// What the example has always shown.
struct Golden {
    id: &'static str,
    processor: &'static str,
    controller: &'static str,
    /// Manufacturer, product, mono and stereo plug-in ids.
    aax: [u32; 4],
    /// `shell::identity` bits: only what the example pinned before (its AAX codes, if any).
    identity: u32,
    params: &'static [&'static str],
}

fn hex(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn check_defaults<P: Plugin + Default + 'static>(golden: &Golden) {
    let plugin = P::default();
    let info = plugin.info();
    assert_eq!(info.id, golden.id);
    assert!(!params_pinned(info.params) && !state_is_keyed(info.params));
    assert_eq!(check_pins(&plugin), []);
    assert_eq!((plugin.clap_id(), plugin.vst3_ids(), plugin.au_ids()), (None, None, None));

    // The shell ABI: class ids, AAX codes, parameter ids.
    let mut shell_info = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(unsafe { shell::write_info::<P>(&mut shell_info) }, 1);
    assert_eq!(hex(&shell_info.processor_cid), golden.processor);
    assert_eq!(hex(&shell_info.controller_cid), golden.controller);
    assert_eq!(
        [shell_info.manufacturer_id, shell_info.product_id, shell_info.plugin_id_mono, shell_info.plugin_id_stereo],
        golden.aax
    );
    assert_eq!(shell::identity::<P>(), golden.identity);
    assert_eq!(shell_info.param_count as usize, golden.params.len());
    for (index, id) in golden.params.iter().enumerate() {
        let index = index as u32;
        assert_eq!(shell::param_host_id::<P>(shell::HOST_VST3, index), index, "the VST3 ParamID is the position");
        assert_eq!(shell::param_host_id::<P>(shell::HOST_AU, index), index, "the AudioUnitParameterID is the position");
        let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
        assert_eq!(unsafe { shell::write_param::<P>(index, &mut desc) }, 1);
        let name = CStr::from_bytes_until_nul(&desc.id).unwrap().to_str().unwrap();
        assert_eq!(name, *id);
        let mut aax = [0u8; 64];
        let length = unsafe { shell::param_aax_id::<P>(index, aax.as_mut_ptr(), aax.len()) };
        assert_eq!(&aax[..length], id.as_bytes(), "the AAX parameter id is the string id");
    }
    assert_eq!(shell::au_code::<P>(shell::AU_SUBTYPE), 0);
    assert_eq!(shell::au_code::<P>(shell::AU_MANUFACTURER), 0);

    // CLAP: the plugin id is PluginInfo::id and a param_id is the position.
    let adapter = ClapAdapter::new(P::default());
    let clap = adapter.as_plugin();
    assert_eq!(unsafe { CStr::from_ptr((*(*clap).desc).id) }.to_str().unwrap(), golden.id);
    let params = unsafe { (*clap).get_extension.unwrap()(clap, c"clap.params".as_ptr()) } as *const clap_plugin_params;
    assert!(!params.is_null());
    assert_eq!(unsafe { (*params).count.unwrap()(clap) } as usize, golden.params.len());
    for index in 0..golden.params.len() as u32 {
        let mut param = unsafe { std::mem::zeroed::<clap_param_info>() };
        assert!(unsafe { (*params).get_info.unwrap()(clap, index, &mut param) });
        assert_eq!(param.id, index);
    }

    // The parameter snapshot is the positional one: magic, version 1, count, one value each.
    let snapshot = encode_params(info, &[]);
    assert_eq!(&snapshot[..6], b"OPST\x01\x00");
    assert_eq!(snapshot.len(), 8 + 4 * golden.params.len());
}
