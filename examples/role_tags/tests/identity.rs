//! The identifiers this example shows a host are the ones it showed before
//! identity pins existed (porting toolkit task 2): see `examples/identity_defaults.rs`.
//! Its own test binary: the shell caches one plugin's descriptor per binary.

include!("../../identity_defaults.rs");

#[test]
fn the_example_keeps_the_identifiers_it_always_had() {
    check_defaults::<example_role_tags::RoleTags>(&Golden {
        id: "com.opengrid.test.role-tags",
        processor: "55FE82CECC3C50968301D9BFFEA6C396",
        controller: "7CAD110CEF80C9A4505A25013AB9D829",
        // The codes the example sets with Plugin::aax_ids: OpTs, RlTg, RlTM, RlTS.
        aax: [0x4F70_5473, 0x526C_5467, 0x526C_544D, 0x526C_5453],
        identity: shell::IDENTITY_AAX_CODES,
        params: &["spare"],
    });
}
