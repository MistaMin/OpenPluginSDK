//! The identifiers this example shows a host are the ones it showed before
//! identity pins existed (porting toolkit task 2): see `examples/identity_defaults.rs`.
//! Its own test binary: the shell caches one plugin's descriptor per binary.

include!("../../identity_defaults.rs");

#[test]
fn the_example_keeps_the_identifiers_it_always_had() {
    check_defaults::<example_expression_synth::ExpressionSynth>(&Golden {
        id: "com.opengrid.example.expression-synth",
        processor: "704132F55267430FEAB0D60E0BD18803",
        controller: "610FFFC68CE0AA48391B0C1BA0EAFA09",
        // No AAX codes of its own: the development codes derived from the vendor and the id.
        aax: [0xA4EE_FFBE, 0xE836_7E7F, 0xF532_6170, 0xE6FF_2F61],
        identity: 0,
        params: &["gain", "tone"],
    });
}
