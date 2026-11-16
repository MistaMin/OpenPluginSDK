//! The identifiers this example shows a host are the ones it showed before
//! identity pins existed (porting toolkit task 2): see `examples/identity_defaults.rs`.
//! Its own test binary: the shell caches one plugin's descriptor per binary.

include!("../../identity_defaults.rs");

#[test]
fn the_example_keeps_the_identifiers_it_always_had() {
    check_defaults::<example_editor::EditorGain>(&Golden {
        id: "com.opengrid.example.editor-gain",
        processor: "34D4F20CA96548A2CCF8B6568B816BBE",
        controller: "4F46AB32F25718AE0303BABE150A0BDF",
        // The codes the example sets with Plugin::aax_ids: OpGd, EdGn, EdGM, EdGS.
        aax: [0x4F70_4764, 0x4564_476E, 0x4564_474D, 0x4564_4753],
        identity: shell::IDENTITY_AAX_CODES,
        params: &["gain", "warm"],
    });
}
