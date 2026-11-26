//! Built-in knob models.

use super::model::{Angular, KnobModel, Material, Part, Plan, Pointer, Segment, Shape};
use Shape::{Dome, Fillet, Flat, Line};

/// Codes accepted by [`preset`].
pub const PRESETS: [&str; 15] =
    ["satin", "bakelite", "spun", "gloss", "strip", "collet", "chicken", "wing", "ivory", "chrome", "phenolic", "wood", "brass", "dome", "dual"];

/// Look up a built-in model by code, see [`PRESETS`].
pub fn preset(code: &str) -> Option<KnobModel> {
    Some(match code {
        "satin" => satin(),
        "bakelite" => bakelite(),
        "spun" => spun(),
        "gloss" => gloss(),
        "strip" => strip(),
        "collet" => collet(),
        "chicken" => chicken(),
        "wing" => wing(),
        "ivory" => ivory(),
        "chrome" => chrome(),
        "phenolic" => phenolic(),
        "wood" => wood(),
        "brass" => brass(),
        "dome" => dome(),
        "dual" => dual(),
        _ => return None,
    })
}

fn seg(shape: Shape, r0: f32, r1: f32, h0: f32, h1: f32, m: Material) -> Segment {
    Segment::new(shape, r0, r1, h0, h1, m)
}
fn white() -> Material {
    Material::plastic(0.93, 0.92, 0.88, 0.45).with_coat(0.5)
}

/// Satin-aluminium cylinder: nearly flat creamy top, a crisp dark line where the top
/// meets the polished wall, thin dark base ring.
fn satin() -> KnobModel {
    // half paint, half metal: keeps the soft form shading of a blasted face
    let top = Material { metallic: 0.5, ..Material::plastic(0.95, 0.94, 0.91, 0.5) }.with_grain(0.012);
    let gap = Material::plastic(0.03, 0.03, 0.035, 0.5);
    let wall = Material::metal(0.93, 0.93, 0.94, 0.26);
    let base = Material::metal(0.22, 0.22, 0.23, 0.30);
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.80, 0.86, 0.855, top),
        seg(Dome, 0.80, 0.845, 0.855, 0.835, top),
        seg(Flat, 0.845, 0.862, 0.81, 0.81, gap),
        seg(Dome, 0.862, 0.91, 0.825, 0.76, wall),
        seg(Line, 0.91, 0.935, 0.76, 0.06, wall),
        seg(Flat, 0.935, 0.965, 0.05, 0.05, base),
        seg(Dome, 0.965, 0.985, 0.05, 0.02, base),
    ])
    .with_edge(0.008)
    .with_pointer(Pointer::groove(0.50, 0.94, 0.10, 0.016, Material::ink()))
}

/// Black scalloped program-equaliser knob: eight-lobed grip with a flat top ring and a
/// recessed centre, on a wide flared skirt; the white index line is on the skirt and again
/// on the top ring, so it shows when the grip hides the skirt behind it.
fn bakelite() -> KnobModel {
    let black = Material::black_polished();
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.31, 0.885, 0.875, black),
        seg(Line, 0.31, 0.345, 0.875, 0.96, black),
        seg(Flat, 0.345, 0.52, 0.96, 0.96, black),
        seg(Dome, 0.52, 0.66, 0.96, 0.82, black),
        seg(Line, 0.66, 0.70, 0.82, 0.30, black),
    ])
    .with_under(vec![seg(Flat, 0.0, 0.60, 0.40, 0.40, black), seg(Line, 0.60, 0.965, 0.40, 0.09, black), seg(Dome, 0.965, 1.0, 0.09, 0.02, black)])
    .with_angular(Angular { r_out: 0.9, ..Angular::scallops(8, 0.13, 0.50) })
    .with_edge(0.02)
    .with_pointer(Pointer::line(0.72, 0.985, 0.075, white()))
    .with_mark(Pointer::line(0.36, 0.60, 0.08, white()))
}

/// Spun-aluminium cap on a knurled black skirt.
fn spun() -> KnobModel {
    let cap = Material::spun_aluminium();
    let rim = Material::metal(0.92, 0.92, 0.93, 0.10);
    let skirt = Material::plastic(0.085, 0.085, 0.09, 0.36);
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.60, 1.02, 1.00, cap),
        seg(Line, 0.60, 0.64, 1.00, 0.95, rim),
        seg(Line, 0.64, 0.66, 0.95, 0.66, rim),
        seg(Line, 0.66, 0.76, 0.66, 0.60, skirt),
        seg(Line, 0.76, 0.96, 0.60, 0.10, skirt),
        seg(Dome, 0.96, 1.0, 0.10, 0.04, skirt),
    ])
    .with_angular(Angular::knurl(40, 0.045, 0.76))
    .with_edge(0.012)
    .with_pointer(Pointer::groove(0.18, 0.60, 0.06, 0.012, Material::ink()))
}

/// Glossy cap in the accent colour with a darker wall of the same hue.
fn gloss() -> KnobModel {
    let cap = Material::plastic(0.5, 0.5, 0.5, 0.5).with_accent(1.0).with_coat(1.0).with_grain(0.02);
    let wall = Material::plastic(0.5, 0.5, 0.5, 0.5).with_accent_shade(1.0, 0.7).with_coat(0.8);
    let foot = Material::plastic(0.03, 0.03, 0.03, 0.6);
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.74, 1.00, 0.95, cap),
        seg(Dome, 0.74, 0.86, 0.95, 0.84, cap),
        seg(Line, 0.86, 0.90, 0.84, 0.07, wall),
        seg(Flat, 0.90, 0.95, 0.06, 0.06, foot),
        seg(Dome, 0.95, 0.97, 0.06, 0.02, foot),
    ])
    .with_edge(0.02)
    .with_pointer(Pointer::line(0.30, 0.86, 0.09, white()))
}

/// Console-strip knob: straight-sided tapered body with a foot ring, flat cap disc in the
/// accent colour and a white line that runs over the cap edge and down the wall.
fn strip() -> KnobModel {
    let cap = Material::plastic(0.5, 0.5, 0.5, 0.45).with_accent(1.0).with_coat(0.55);
    let body = Material::plastic(0.25, 0.25, 0.26, 0.48).with_sheen(0.2);
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.50, 1.162, 1.15, cap),
        seg(Flat, 0.50, 0.52, 1.13, 1.13, body),
        seg(Flat, 0.52, 0.57, 1.15, 1.15, body),
        seg(Line, 0.57, 0.61, 1.15, 1.10, body),
        seg(Line, 0.61, 0.80, 1.10, 0.14, body),
        seg(Flat, 0.80, 0.86, 0.12, 0.12, body),
        seg(Line, 0.86, 0.885, 0.12, 0.02, body),
    ])
    .with_angular(Angular { r_out: 0.80, ..Angular::flutes(26, 0.018, 0.64) })
    .with_edge(0.008)
    .with_pointer(Pointer::line(0.20, 0.80, 0.08, Material::white_paint()))
}

/// Grey ribbed cap with a flat insert inside a wide polished bezel that carries the index.
fn collet() -> KnobModel {
    let cap = Material::plastic(0.42, 0.43, 0.45, 0.55);
    let face = Material::plastic(0.56, 0.57, 0.59, 0.5).with_coat(0.3);
    let bezel = Material::metal(0.96, 0.96, 0.97, 0.07);
    KnobModel::new(vec![
        seg(Flat, 0.0, 0.40, 0.97, 0.97, face),
        seg(Line, 0.40, 0.425, 0.97, 1.0, cap),
        seg(Dome, 0.425, 0.56, 1.0, 0.94, cap),
        seg(Line, 0.56, 0.60, 0.94, 0.26, cap),
    ])
    .with_under(vec![
        seg(Flat, 0.0, 0.62, 0.26, 0.26, bezel),
        seg(Line, 0.62, 0.68, 0.26, 0.32, bezel),
        seg(Dome, 0.68, 0.955, 0.32, 0.08, bezel),
        seg(Dome, 0.955, 0.985, 0.08, 0.02, bezel),
    ])
    .with_angular(Angular::flutes(18, 0.07, 0.50))
    .with_edge(0.012)
    .with_pointer(Pointer::groove(0.66, 0.97, 0.10, 0.012, Material::ink()))
    .with_mark(Pointer::line(0.08, 0.39, 0.09, Material::ink()))
}

/// Black chicken-head pointer: a teardrop with a flat crest that falls away to the tip
/// and a white inlay line along it, on a short knurled skirt.
fn chicken() -> KnobModel {
    let black = Material::black_polished();
    let beak = Part::new(
        vec![
            seg(Flat, 0.0, 0.70, 0.96, 0.96, black),
            seg(Line, 0.70, 0.92, 0.96, 0.80, black),
            seg(Line, 0.92, 0.97, 0.80, 0.70, black),
            seg(Line, 0.97, 1.0, 0.70, 0.22, black),
        ],
        Plan::Bar { tip: 1.0, tail: 0.0, tip_width: 0.055, tail_width: 0.50, rounding: 0.08 },
    );
    KnobModel::new(vec![
        seg(Flat, 0.0, 0.60, 0.26, 0.26, black),
        seg(Line, 0.60, 0.64, 0.26, 0.22, black),
        seg(Line, 0.64, 0.68, 0.22, 0.03, black),
    ])
    .with_under(vec![seg(Flat, 0.0, 0.43, 0.96, 0.96, black), seg(Line, 0.43, 0.50, 0.96, 0.86, black), seg(Line, 0.50, 0.53, 0.86, 0.22, black)])
    .with_angular(Angular::knurl(38, 0.03, 0.60))
    .with_part(beak)
    .with_edge(0.016)
    .with_pointer(Pointer::line(0.0, 0.975, 0.05, white()))
}

/// Die-cast winged pointer knob: round body with a large recessed accent insert, a long
/// fin that stands proud of the rim from the edge of the insert out to the tip, and
/// three short lugs, on a thin flange.
fn wing() -> KnobModel {
    let cast = Material::die_cast();
    let disc = Material::plastic(0.5, 0.5, 0.5, 0.5).with_accent(1.0).with_coat(0.35).with_grain(0.03);
    let fin = Part::new(
        vec![seg(Flat, 0.0, 0.93, 0.88, 0.88, cast), seg(Line, 0.93, 0.975, 0.88, 0.78, cast), seg(Line, 0.975, 1.0, 0.78, 0.05, cast)],
        Plan::Bar { tip: 1.0, tail: -0.43, tip_width: 0.11, tail_width: 0.27, rounding: 0.06 },
    );
    let lugs = Part::new(
        vec![seg(Flat, 0.0, 0.58, 0.56, 0.56, cast), seg(Line, 0.58, 0.63, 0.56, 0.48, cast), seg(Line, 0.63, 0.66, 0.48, 0.05, cast)],
        Plan::Round,
    )
    .with_angular(Angular::teeth(4, 0.24, 0.20, 0.25));
    KnobModel::new(vec![
        seg(Flat, 0.0, 0.47, 0.78, 0.78, cast),
        seg(Line, 0.47, 0.50, 0.78, 0.75, cast),
        seg(Line, 0.50, 0.52, 0.75, 0.12, cast),
        seg(Flat, 0.52, 0.57, 0.10, 0.10, cast),
        seg(Line, 0.57, 0.595, 0.10, 0.02, cast),
    ])
    .with_part(fin)
    .with_part(lugs)
    .with_insert(0.42, 0.04, disc)
    .with_edge(0.012)
    .with_pointer(Pointer::groove(0.50, 0.95, 0.045, 0.014, Material::ink()))
}

/// Cream fluted vintage knob on a flared skirt, dark pointer line.
fn ivory() -> KnobModel {
    let cream = Material::cream();
    KnobModel::new(vec![seg(Dome, 0.0, 0.56, 0.96, 0.93, cream), seg(Dome, 0.56, 0.74, 0.93, 0.80, cream), seg(Line, 0.74, 0.79, 0.80, 0.12, cream)])
        .with_under(vec![seg(Flat, 0.0, 0.80, 0.16, 0.16, cream), seg(Dome, 0.80, 0.92, 0.16, 0.03, cream)])
        .with_angular(Angular::flutes(18, 0.05, 0.62))
        .with_edge(0.02)
        .with_pointer(Pointer::line(0.16, 0.70, 0.055, Material::plastic(0.10, 0.07, 0.05, 0.6)))
}

/// Chrome skirted knob with a small cap.
fn chrome() -> KnobModel {
    let cr = Material::chrome();
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.40, 1.00, 0.97, cr),
        seg(Dome, 0.40, 0.50, 0.97, 0.88, cr),
        seg(Line, 0.50, 0.54, 0.88, 0.52, cr),
        seg(Fillet, 0.54, 0.93, 0.52, 0.10, cr),
        seg(Dome, 0.93, 0.99, 0.10, 0.03, cr),
    ])
    .with_edge(0.022)
    .with_pointer(Pointer::groove(0.56, 0.95, 0.11, 0.012, Material::ink()))
    .with_mark(Pointer::groove(0.06, 0.38, 0.10, 0.012, Material::ink()))
}

/// Plain black skirted pointer knob: white line on the cap and again on the skirt.
fn phenolic() -> KnobModel {
    let black = Material::black_polished();
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.42, 0.84, 0.82, black),
        seg(Dome, 0.42, 0.52, 0.82, 0.74, black),
        seg(Line, 0.52, 0.58, 0.74, 0.46, black),
        seg(Line, 0.58, 0.93, 0.46, 0.12, black),
        seg(Dome, 0.93, 0.975, 0.12, 0.02, black),
    ])
    .with_edge(0.018)
    .with_pointer(Pointer::line(0.08, 0.46, 0.06, white()))
    .with_mark(Pointer::line(0.64, 0.93, 0.06, white()))
}

/// Lacquered walnut knob: low crowned top with a turned shoulder, brass foot ring and a
/// pale inlaid pointer line.
fn wood() -> KnobModel {
    let walnut = Material::walnut().with_coat(0.35).with_roughness(0.5);
    let foot = Material::brass().with_roughness(0.25);
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.70, 0.90, 0.87, walnut),
        seg(Dome, 0.70, 0.84, 0.87, 0.76, walnut),
        seg(Line, 0.84, 0.90, 0.76, 0.10, walnut),
        seg(Flat, 0.90, 0.945, 0.08, 0.08, foot),
        seg(Dome, 0.945, 0.97, 0.08, 0.02, foot),
    ])
    .with_edge(0.016)
    .with_pointer(Pointer::line(0.30, 0.82, 0.065, Material::plastic(0.90, 0.84, 0.68, 0.5).with_coat(0.5)))
}

/// Knurled brass knob with a turned face.
fn brass() -> KnobModel {
    let face = Material::brass().with_roughness(0.12).with_aniso(0.4).with_rings(0.6);
    let side = Material::brass();
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.70, 0.86, 0.85, face),
        seg(Line, 0.70, 0.76, 0.85, 0.80, side),
        seg(Line, 0.76, 0.80, 0.80, 0.80, side),
        seg(Line, 0.80, 0.90, 0.80, 0.14, side),
        seg(Flat, 0.90, 0.945, 0.12, 0.12, side),
        seg(Dome, 0.945, 0.97, 0.12, 0.02, side),
    ])
    .with_angular(Angular { r_out: 0.9, ..Angular::knurl(44, 0.04, 0.80) })
    .with_edge(0.012)
    .with_pointer(Pointer::groove(0.22, 0.70, 0.055, 0.012, Material::ink()))
}

/// Moulded black dome knob: a smooth dome standing on a stepped skirt ring, with a clear
/// white dot as pointer.
fn dome() -> KnobModel {
    let body = Material::plastic(0.09, 0.09, 0.10, 0.40).with_coat(0.7).with_grain(0.02);
    KnobModel::new(vec![
        seg(Dome, 0.0, 0.70, 0.86, 0.46, body),
        seg(Line, 0.70, 0.725, 0.46, 0.27, body),
        seg(Flat, 0.725, 0.76, 0.26, 0.26, body),
        seg(Dome, 0.76, 0.93, 0.27, 0.10, body),
        seg(Line, 0.93, 0.96, 0.10, 0.02, body),
    ])
    .with_edge(0.02)
    .with_pointer(Pointer::dot(0.47, 0.17, 0.012, Material::white_paint()))
}

/// Two-tier concentric knob: wide knurled lower ring, smaller satin cap on top.
fn dual() -> KnobModel {
    let lower = Material::anodised(0.16, 0.16, 0.17).with_roughness(0.38);
    let upper = Material::satin_aluminium();
    KnobModel::two_tier(0.40, 0.56, 0.98, lower, upper)
        .with_angular(Angular::knurl(48, 0.035, 0.90))
        .with_edge(0.012)
        .with_pointer(Pointer::groove(0.14, 0.56, 0.06, 0.012, Material::ink()))
        .with_mark(Pointer::line(0.66, 0.93, 0.045, Material::white_paint()))
}
