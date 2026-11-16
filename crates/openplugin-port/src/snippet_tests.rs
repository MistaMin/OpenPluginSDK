use crate::manifest::*;
use crate::snippet::*;

fn param(points: &[(f64, f64, &str)], min: f64, max: f64) -> Param {
    Param {
        id: 7,
        name: "Cutoff".into(),
        min,
        max,
        default: min,
        flags: vec!["automatable".into()],
        samples: points.iter().map(|&(n, p, t)| Sample { normalized: n, plain: p, text: t.into() }).collect(),
        ..Param::default()
    }
}

#[test]
fn value_strings_parse() {
    assert_eq!(parse_value("-12.5 dB"), Some((-12.5, "dB".into())));
    assert_eq!(parse_value("1.5 kHz"), Some((1500.0, "Hz".into())));
    assert_eq!(parse_value("50%"), Some((50.0, "%".into())));
    assert_eq!(parse_value("4:1"), Some((4.0, ":1".into())));
    assert_eq!(parse_value("-inf dB"), None);
    assert_eq!(parse_value("Off"), None);
}

#[test]
fn a_line_and_a_curve_are_told_apart() {
    let line: Vec<(f64, f64)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, 10.0 + 90.0 * t)).collect();
    let f = fit(&line).unwrap();
    assert!(f.linear_error < 1e-9 && !is_log(&f));
    let curve: Vec<(f64, f64)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, 20.0 * (1000.0f64).powf(t))).collect();
    let f = fit(&curve).unwrap();
    assert!(f.log_error.unwrap() < 1e-9 && is_log(&f));
}

#[test]
fn a_plain_log_range_gives_a_log_taper() {
    let owned: Vec<(f64, f64)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, 20.0 * (1000.0f64).powf(t))).collect();
    let pts: Vec<(f64, f64, &str)> = owned.iter().map(|&(a, b)| (a, b, "")).collect();
    let i = infer(&Plugin { params: vec![param(&pts, 20.0, 20000.0)], ..Plugin::default() });
    assert_eq!(i[0].taper, "Log");
    assert_eq!((i[0].min, i[0].max), (20.0, 20000.0));
}

#[test]
fn value_strings_reveal_a_hidden_log_display() {
    // Plain 0..1, linear in the position, but the text runs 20 Hz to 20 kHz on a curve.
    let texts = ["20 Hz", "112 Hz", "632 Hz", "3.56 kHz", "20 kHz"];
    let pts: Vec<(f64, f64, &str)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().zip(texts).map(|(&t, s)| (t, t, s)).collect();
    let i = infer(&Plugin { params: vec![param(&pts, 0.0, 1.0)], ..Plugin::default() });
    assert_eq!(i[0].taper, "Log");
    assert_eq!((i[0].min, i[0].max), (20.0, 20000.0));
    assert_eq!(i[0].unit, "Hz");
}

#[test]
fn no_text_means_a_commented_linear_guess() {
    let pts: Vec<(f64, f64, &str)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, t, "")).collect();
    let i = infer(&Plugin { params: vec![param(&pts, 0.0, 1.0)], ..Plugin::default() });
    assert_eq!(i[0].taper, "Linear");
    assert!(i[0].notes.iter().any(|n| n.contains("guessed")));
}

#[test]
fn choices_toggles_and_unique_ids() {
    let mut choice = param(&[(0.0, 0.0, ""), (1.0, 2.0, "")], 0.0, 2.0);
    choice.name = "Mode".into();
    choice.step_count = 2;
    choice.step_texts = vec!["Clean".into(), "Warm".into(), "Hot".into()];
    let mut toggle = param(&[(0.0, 0.0, "Off"), (1.0, 1.0, "On")], 0.0, 1.0);
    toggle.name = "Mode".into();
    toggle.step_count = 1;
    toggle.step_texts = vec!["Off".into(), "On".into()];
    let i = infer(&Plugin { params: vec![choice, toggle], ..Plugin::default() });
    assert_eq!((i[0].unit, i[0].choices.len(), i[0].max), ("Choice", 3, 2.0));
    assert_eq!(i[1].unit, "Toggle");
    assert_eq!((i[0].id.as_str(), i[1].id.as_str()), ("mode", "mode_2"));
}

// ---------- identity pins ----------

fn ids(list: &[u32]) -> Vec<Param> {
    list.iter().map(|&id| Param { id, name: format!("P{id}"), max: 1.0, ..Param::default() }).collect()
}

#[test]
fn a_table_numbered_by_position_needs_no_pins() {
    for format in ["clap", "vst3", "au"] {
        assert_eq!(pins(format, &ids(&[0, 1, 2])), vec![Pin::Position; 3], "{format}");
    }
    // A scan of an AAX bundle reads no parameter ids.
    assert_eq!(pins("aax", &ids(&[5, 9])), vec![Pin::Position; 2]);
}

#[test]
fn one_id_that_is_not_a_position_pins_the_whole_table() {
    assert_eq!(
        pins("clap", &ids(&[0, 7, 2])),
        [Pin::Id { method: "with_clap_id", id: 0 }, Pin::Id { method: "with_clap_id", id: 7 }, Pin::Id { method: "with_clap_id", id: 2 }]
    );
    assert_eq!(pins("vst3", &ids(&[1, 0])), [Pin::Id { method: "with_vst3_id", id: 1 }, Pin::Id { method: "with_vst3_id", id: 0 }]);
    assert_eq!(pins("au", &ids(&[0x6761_696E])), [Pin::Id { method: "with_au_id", id: 0x6761_696E }]);
}

#[test]
fn ids_a_format_cannot_carry_are_named_not_pinned() {
    // The SDK shell's own VST3 bypass after two positions: nothing else needs a pin.
    let own = pins("vst3", &ids(&[0, 1, 0x4259_5053]));
    assert_eq!(&own[..2], [Pin::Position, Pin::Position]);
    assert!(matches!(&own[2], Pin::Unpinnable(why) if why.contains("bypass")));
    // The same id beside real pins, a MIDI controller id, an id above the VST3 range, a repeated id.
    let mixed = pins("vst3", &ids(&[40, 0x4259_5053, 0x4D49_4400, 0x8000_0000, 40]));
    assert_eq!(mixed[0], Pin::Id { method: "with_vst3_id", id: 40 });
    assert!(mixed[1..].iter().all(|pin| matches!(pin, Pin::Unpinnable(_))), "{mixed:?}");
    assert!(matches!(&pins("clap", &ids(&[u32::MAX]))[0], Pin::Unpinnable(_)));
    // The VST3 shell's ids mean nothing in CLAP.
    assert_eq!(pins("clap", &ids(&[0x4259_5053])), [Pin::Id { method: "with_clap_id", id: 0x4259_5053 }]);
}

fn manifest(format: &str, params: Vec<Param>, ids: Ids) -> Manifest {
    Manifest {
        schema_version: SCHEMA_VERSION,
        tool: "openplugin-port test".into(),
        source: Source { format: format.into(), path: "Old.plugin".into(), index: 0, count: 1, bundle_id: None },
        plugin: Plugin { name: "Old".into(), vendor: "Acme".into(), version: "2.1.0".into(), params, ids, ..Plugin::default() },
    }
}

#[test]
fn the_generated_table_carries_the_pins() {
    let text = render(&manifest("vst3", ids(&[7, 0x6761_696E]), Ids::default()));
    assert!(text.contains(".with_vst3_id(7),"), "{text}");
    assert!(text.contains(".with_vst3_id(0x6761696E),"), "{text}");
    assert!(text.contains("each parameter is pinned to its id"), "{text}");
    assert!(text.contains("check_pins"), "{text}");
    let plain = render(&manifest("clap", ids(&[0, 1]), Ids::default()));
    assert!(!plain.contains("with_clap_id") && plain.contains("nothing to pin"), "{plain}");
}

#[test]
fn the_generated_identity_maps_onto_the_plugin_pins() {
    let ids_of = |controller: Option<&str>| Ids {
        vst3: Some(Vst3Ids {
            class_id: "A1B2C3D4E5F60718293A4B5C6D7E8F90".into(),
            controller_class_id: controller.map(str::to_string),
            ..Vst3Ids::default()
        }),
        au: Some(AuIds { component_type: "aumf".into(), subtype: "OldF".into(), manufacturer: "Acme".into(), ..AuIds::default() }),
        aax: Some(AaxIds { manufacturer: "Acme".into(), product: "OldF".into(), plugin_ids: vec!["OldM".into(), "OldS".into()], found_in: "page-table".into() }),
        clap: None,
    };
    let text = render(&manifest("vst3", ids(&[0, 1]), ids_of(Some("0F1E2D3C4B5A69788796A5B4C3D2E1F0"))));
    assert!(text.contains("fn vst3_ids(&self) -> Option<Vst3Ids>"), "{text}");
    assert!(text.contains("Vst3Ids::from_hex(\"A1B2C3D4E5F60718293A4B5C6D7E8F90\", \"0F1E2D3C4B5A69788796A5B4C3D2E1F0\")"), "{text}");
    assert!(text.contains("fn au_ids(&self) -> Option<AuIds>"), "{text}");
    assert!(text.contains("Some(AuIds { subtype: *b\"OldF\", manufacturer: *b\"Acme\" })"), "{text}");
    let build = text.lines().find(|line| line.starts_with("// scripts/build_plugin.sh")).expect("the build line");
    assert!(build.contains("--au-type aumf"), "{build}");
    assert!(!build.contains("--au-subtype"), "the pinned codes need no build option: {build}");
    assert!(text.contains("Some(AaxIds { manufacturer: *b\"Acme\", product: *b\"OldF\", mono: *b\"OldM\", stereo: *b\"OldS\" })"), "{text}");
    assert!(!text.contains("cannot pin") && !text.contains("task 2"), "nothing is left that the SDK cannot pin: {text}");
    // A single-component source has no controller class to keep.
    let single = render(&manifest("vst3", ids(&[0, 1]), ids_of(None)));
    assert!(single.contains("Vst3Ids::processor_from_hex(\"A1B2C3D4E5F60718293A4B5C6D7E8F90\")"), "{single}");
}

/// The generated pins are real code: the same methods, checked by the SDK.
#[test]
fn the_pins_the_snippet_writes_are_the_sdks() {
    use openplugin_core::identity::{param_host_ids, PinFormat, Vst3Ids as CoreVst3Ids};
    use openplugin_core::{ParamInfo, Unit};
    let table = [
        ParamInfo::new("a", "A", 0.0, 1.0, 0.0, Unit::None).with_vst3_id(7),
        ParamInfo::new("b", "B", 0.0, 1.0, 0.0, Unit::None).with_vst3_id(0x6761696E),
    ];
    assert_eq!(param_host_ids(&table, PinFormat::Vst3), Ok(vec![7, 0x6761_696E]));
    assert!(CoreVst3Ids::from_hex("A1B2C3D4E5F60718293A4B5C6D7E8F90", "0F1E2D3C4B5A69788796A5B4C3D2E1F0").is_some_and(|ids| ids.is_valid()));
}
