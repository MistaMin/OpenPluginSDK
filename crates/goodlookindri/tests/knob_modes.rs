use goodlookindri::{color::Color, design::*, draw, gallery, knob3d, looks, scene::{Cmd, Point, Scene}};

fn drawing(code: &str, mode: KnobMode) -> Scene {
    let mut scene = Scene::new();
    draw::knob_named_mode(&mut scene, code, Point::new(60.0, 60.0), 18.0, 0.4, Color(0xffc96f4a), false, mode);
    scene
}

#[test]
fn prefixes_roundtrip_and_validate() {
    for style in KNOB_STYLES {
        for (prefix, mode) in [("", KnobMode::Classic), ("flat_", KnobMode::Flat), ("lit_", KnobMode::Lit)] {
            let name = format!("{prefix}{}", style.code);
            assert_eq!(parse_style(&name), (mode, style.code));
            assert_eq!(find_style(&name).unwrap().style, style.style);
            assert_eq!(is_flat_style(&name), mode == KnobMode::Flat);
            let item = Item::new("gain", "gain", "Gain", &name, "#c96f4a", 0.0, 0.0, 60.0, 60.0, 11.0);
            assert_eq!(read_design(&write_design(&[item.clone()]).unwrap()).unwrap(), vec![item]);
        }
    }
    assert_eq!(parse_style("flat"), (KnobMode::Flat, "Brit"));
    assert!(looks::valid_look(&looks::Look {
        parameter: "gain".into(), choice: "one".into(), style: "lit_Mtl".into(),
        colour: String::new(), faceplate: String::new(), plate: String::new(),
    }));
    for code in knob3d::PRESETS {
        assert!(find_style(&format!("lit_{code}")).is_some());
        assert!(drawing(code, KnobMode::Lit).cmds.iter().any(|c| matches!(c, Cmd::Image { .. })));
    }
    assert_eq!(KNOB_STYLES.len(), 47);
}

#[test]
fn lit_catalogue_and_fallback() {
    assert!(gallery::every_style_draws_mode(KnobMode::Lit));
    for style in KNOB_STYLES {
        if knob3d::style_model(style.code).is_none() {
            assert_eq!(drawing(style.code, KnobMode::Lit).cmds, drawing(style.code, KnobMode::Classic).cmds);
        }
    }
}

#[test]
fn lit_image_identity_and_placement() {
    let a = drawing("lit_Mtl", KnobMode::Classic);
    let b = drawing("Mtl", KnobMode::Lit);
    assert_eq!(a.cmds, b.cmds);
    let images: Vec<_> = a.cmds.iter().filter_map(|cmd| match cmd {
        Cmd::Image { img, dest, opacity } => Some((img, dest, opacity)), _ => None,
    }).collect();
    assert_eq!(images.len(), 1);
    let (img, dest, opacity) = images[0];
    let sprite = knob3d::KnobCache::global().get(&knob3d::style_model("Mtl").unwrap(), &knob3d::Rig::studio(), 36.0, 0.4, Color(0xffc96f4a), Color(0xff808080));
    assert_eq!(dest.w, img.w as f32 / 2.0);
    assert_eq!(dest.h, img.h as f32 / 2.0);
    assert_eq!(dest.x + sprite.origin.0 / 2.0, 60.0);
    assert_eq!(dest.y + sprite.origin.1 / 2.0, 60.0);
    assert_eq!(*opacity, 1.0);
    assert_eq!(drawing("flat_Mtl", KnobMode::Lit).cmds, drawing("Mtl", KnobMode::Flat).cmds);
}

#[test]
fn every_physical_style_has_its_own_lit_model() {
    let displays = ["Seg", "Vfd", "Cyb", "Arc", "Brg", "Neo", "Hex", "Yel", "Glw", "Grd", "Dsh", "Led"];
    assert_eq!(knob3d::STYLE_CODES.len() + displays.len(), KNOB_STYLES.len());
    for code in displays {
        assert!(find_style(code).is_some(), "{code}");
        assert!(knob3d::style_model(code).is_none(), "{code}");
    }
    let sprites: Vec<_> = knob3d::STYLE_CODES.iter().map(|code| {
        assert!(find_style(code).is_some(), "{code}");
        let model = knob3d::style_model(code).unwrap_or_else(|| panic!("{code} has no lit model"));
        knob3d::render(&model, &knob3d::Rig::studio(), 40.0, 0.4, Color(0xffc96f4a))
    }).collect();
    for i in 0..sprites.len() {
        for j in 0..i {
            assert!(sprites[i].px != sprites[j].px, "{} and {} look the same", knob3d::STYLE_CODES[i], knob3d::STYLE_CODES[j]);
        }
    }
    for style in KNOB_STYLES {
        if !style.legacy.is_empty() {
            assert_eq!(knob3d::style_model(style.legacy), knob3d::style_model(style.code), "{}", style.legacy);
        }
        // every catalogue entry is either a physical style with a model or a display
        assert_eq!(knob3d::style_model(style.code).is_some(), !displays.contains(&style.code), "{}", style.code);
    }
    assert!(drawing("Wd", KnobMode::Lit).cmds.iter().any(|c| matches!(c, Cmd::Image { .. })));
}
