//! Design model ported from GoodLookinUI `Design.h`.
//! CSV text matches the C++ writer for the published fixtures (integer geometry, optional marks column).

use std::collections::BTreeSet;
use std::fmt;

/// Rendering tier; it does not add entries to the style catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnobMode { Flat, Classic, Lit }

/// Explicit prefixes override the caller's mode. Bare `flat` is the legacy Brit alias.
pub fn parse_style(code: &str) -> (KnobMode, &str) {
    if let Some(code) = code.strip_prefix("flat_") { (KnobMode::Flat, code) }
    else if let Some(code) = code.strip_prefix("lit_") { (KnobMode::Lit, code) }
    else if code == "flat" { (KnobMode::Flat, "Brit") }
    else { (KnobMode::Classic, code) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnobStyle { 
    Brit, N, A, FS, Mtl, Wd, Bk, Pie, Snk, Slv, Rd, Ptr, Dm, Sq, Seg, Vfd, Cyb, Fat, Pag, Lvr, Tab, Wh, Knl, Skt, Rib, Chr, Arc, Brg, Neo, Hex, Yel, Key, Brz, Chk, Ivr, Mpl, Led, Dbl, Mini, Tl, Glw, Grd, Dsh, Mic, Ebn, Wnt, Rck
}

#[derive(Clone, Copy, Debug)]
pub struct StyleInfo { pub style: KnobStyle, pub code: &'static str, pub legacy: &'static str, pub description: &'static str }

pub static KNOB_STYLES: &[StyleInfo] = &[

    StyleInfo { style: KnobStyle::Brit, code: "Brit", legacy: "console", description: "Black fluted grip, large coloured flat cap, recessed white pointer" },
    StyleInfo { style: KnobStyle::N, code: "N", legacy: "", description: "Chrome collar, grey fluted grip, flat cap, dotted scale ring" },
    StyleInfo { style: KnobStyle::A, code: "A", legacy: "", description: "Flared silver crown with a large flat matte cap (any colour, dark for charcoal), dotted ring" },
    StyleInfo { style: KnobStyle::FS, code: "FS", legacy: "", description: "Pale-grey skirt, coloured cap, black pointer line" },
    StyleInfo { style: KnobStyle::Mtl, code: "Mtl", legacy: "metal", description: "Knurled aluminium body, brushed top, dark pointer line" },
    StyleInfo { style: KnobStyle::Wd, code: "Wd", legacy: "", description: "Turned wooden knob, grain, inlaid pointer line" },
    StyleInfo { style: KnobStyle::Bk, code: "Bk", legacy: "bakelite", description: "Black bakelite teardrop pointer over a ribbed base" },
    StyleInfo { style: KnobStyle::Pie, code: "Pie", legacy: "", description: "White serrated pie-crust skirt, black cap, pointer line" },
    StyleInfo { style: KnobStyle::Snk, code: "Snk", legacy: "", description: "Matte black cylinder, fixed tick ring, pointer line" },
    StyleInfo { style: KnobStyle::Slv, code: "Slv", legacy: "ivory", description: "Silver-grey cylinder, dark skirt ring with dots, black pointer" },
    StyleInfo { style: KnobStyle::Rd, code: "Rd", legacy: "", description: "Round black body, coloured cap, tick ring" },
    StyleInfo { style: KnobStyle::Ptr, code: "Ptr", legacy: "", description: "Flat-top silver pointer knob (tapered), dark pointer line" },
    StyleInfo { style: KnobStyle::Dm, code: "Dm", legacy: "", description: "Chunky glossy domed cap in the item colour, pointer dot" },
    StyleInfo { style: KnobStyle::Sq, code: "Sq", legacy: "", description: "Square rounded keycap with a dished top and legend line" },
    StyleInfo { style: KnobStyle::Seg, code: "Seg", legacy: "", description: "Digital: dark cylinder inside a lit segmented arc (last segments red)" },
    StyleInfo { style: KnobStyle::Vfd, code: "Vfd", legacy: "", description: "Digital: phosphor-glass disc with a vector fan of lit lines" },
    StyleInfo { style: KnobStyle::Cyb, code: "Cyb", legacy: "", description: "Digital: chamfered dark frame, glowing outline, notched ring" },
    StyleInfo { style: KnobStyle::Fat, code: "Fat", legacy: "", description: "Large black skirt with a thick white pointer running to the edge" },
    StyleInfo { style: KnobStyle::Pag, code: "Pag", legacy: "", description: "Black ribbed octagonal bakelite with a glossy centre dome" },
    StyleInfo { style: KnobStyle::Lvr, code: "Lvr", legacy: "", description: "Black bar lever selector on a round hub" },
    StyleInfo { style: KnobStyle::Tab, code: "Tab", legacy: "", description: "Small black body with a short tab pointer" },
    StyleInfo { style: KnobStyle::Wh, code: "Wh", legacy: "", description: "Black knurled skirt with a white cap and black line" },
    StyleInfo { style: KnobStyle::Knl, code: "Knl", legacy: "", description: "Dark knurled body with a brushed aluminium cap and pointer" },
    StyleInfo { style: KnobStyle::Skt, code: "Skt", legacy: "", description: "Black skirt with a bright pointer line running down its side" },
    StyleInfo { style: KnobStyle::Rib, code: "Rib", legacy: "", description: "Large ribbed knob moulded in the item colour" },
    StyleInfo { style: KnobStyle::Chr, code: "Chr", legacy: "", description: "Chrome skirt, glossy dark cap, white pointer, dotted ring" },
    StyleInfo { style: KnobStyle::Arc, code: "Arc", legacy: "", description: "Digital: continuous lit arc band (amber to red) around a dark cap" },
    StyleInfo { style: KnobStyle::Brg, code: "Brg", legacy: "", description: "Digital: rising bar-graph arc, bars grow with value" },
    StyleInfo { style: KnobStyle::Neo, code: "Neo", legacy: "", description: "Digital: thin neon ring with glowing value arc and dot" },
    StyleInfo { style: KnobStyle::Hex, code: "Hex", legacy: "", description: "Digital: hexagon frame with tinted glass and pointer" },
    StyleInfo { style: KnobStyle::Yel, code: "Yel", legacy: "", description: "Digital: rugged rounded chassis frame in the item colour, black screen, cyan pointer" },
    StyleInfo { style: KnobStyle::Key, code: "Key", legacy: "", description: "Round dished keycap with a coloured legend" },
    StyleInfo { style: KnobStyle::Brz, code: "Brz", legacy: "", description: "Polished brass knurled knob, black pointer" },
    StyleInfo { style: KnobStyle::Chk, code: "Chk", legacy: "", description: "Black chicken-head pointer knob with a white line" },
    StyleInfo { style: KnobStyle::Ivr, code: "Ivr", legacy: "", description: "Cream bakelite fluted knob, dark pointer" },
    StyleInfo { style: KnobStyle::Mpl, code: "Mpl", legacy: "", description: "Light maple wooden knob with inlaid pointer" },
    StyleInfo { style: KnobStyle::Led, code: "Led", legacy: "", description: "Digital: dark cylinder inside a ring of lit LED dots" },
    StyleInfo { style: KnobStyle::Dbl, code: "Dbl", legacy: "", description: "Stacked two-tier knob: silver outer ring, black inner knob" },
    StyleInfo { style: KnobStyle::Mini, code: "Mini", legacy: "", description: "Small trimmer knob with a white slot and tick ring" },
    StyleInfo { style: KnobStyle::Tl, code: "Tl", legacy: "", description: "Tall black knurled knob with a white line" },
    StyleInfo { style: KnobStyle::Glw, code: "Glw", legacy: "", description: "Neon: dark glass disc, glowing value ring and glowing pointer" },
    StyleInfo { style: KnobStyle::Grd, code: "Grd", legacy: "", description: "Neon: perspective-grid face with a glowing pointer and dotted ring" },
    StyleInfo { style: KnobStyle::Dsh, code: "Dsh", legacy: "", description: "Dash: ramp of bars filling to the value with a needle" },
    StyleInfo { style: KnobStyle::Mic, code: "Mic", legacy: "", description: "Translucent crown pointer knob in the item colour with a white indicator, dotted ring" },
    StyleInfo { style: KnobStyle::Ebn, code: "Ebn", legacy: "", description: "Ebony wood knob with a brass cap and an inlaid pointer line" },
    StyleInfo { style: KnobStyle::Wnt, code: "Wnt", legacy: "", description: "Walnut wood top inside a chrome collar, pointer line" },
    StyleInfo { style: KnobStyle::Rck, code: "Rck", legacy: "", description: "Small black rack-unit knob: fluted skirt, satin top, soft contact shadow, bright pointer line" },
];


pub fn find_style(name: &str) -> Option<&'static StyleInfo> {
    let (mode, code) = parse_style(name);
    // Direct lit presets resolve to a representative catalogue entry for validation.
    let code = if mode == KnobMode::Lit {
        match code {
            "satin" | "spun" => "Mtl", "bakelite" => "Bk", "gloss" => "Rd",
            "strip" => "Brit", "collet" => "Dbl", "chicken" => "Chk",
            "wing" => "A", "ivory" => "Ivr", "chrome" => "Chr", "phenolic" => "Snk",
            "wood" => "Wd", "brass" => "Brz", "dome" => "Dm", "dual" => "Dbl",
            _ => code,
        }
    } else { code };
    KNOB_STYLES.iter().find(|s| s.code == code || (!s.legacy.is_empty() && s.legacy == code))
}
pub fn is_flat_style(name: &str) -> bool { parse_style(name).0 == KnobMode::Flat }

#[derive(Clone, Copy, Debug)]
pub struct Faceplate {
    pub code: &'static str, pub name: &'static str,
    pub bg_top: u32, pub bg_bottom: u32, pub panel_top: u32, pub panel_bottom: u32,
    pub text: u32, pub text_mid: u32, pub accent: u32, pub bg_fill: u32, pub border: u32,
    pub title_bar: u32, pub rail: u32, pub footer: u32,
    /// 0 flat, 1 brushed, 2 wood, 3 scanlines, 4 hazard, 5 grid, 6 neon edge, 7 synth horizon.
    pub finish: i32,
}
pub static FACEPLATES: &[Faceplate] = &[

    Faceplate { code: "Graphite", name: "Graphite", bg_top: 0xff242c31, bg_bottom: 0xff151c20, panel_top: 0xff394247, panel_bottom: 0xff2a3034, text: 0xffdde1d5, text_mid: 0xff909c9f, accent: 0xffc7a66a, bg_fill: 0xff10171b, border: 0xff485156, title_bar: 0xff121b20, rail: 0xff0c1114, footer: 0xff526065, finish: 0 },
    Faceplate { code: "Black", name: "Matte black", bg_top: 0xff1a1a1c, bg_bottom: 0xff0b0b0c, panel_top: 0xff2a2a2d, panel_bottom: 0xff18181a, text: 0xffe6e6e6, text_mid: 0xff8b8b8e, accent: 0xffd0d0d0, bg_fill: 0xff070708, border: 0xff505052, title_bar: 0xff09090a, rail: 0xff060606, footer: 0xff5e5e60, finish: 0 },
    Faceplate { code: "Navy", name: "Navy blue", bg_top: 0xff1c2d5a, bg_bottom: 0xff0e1a3c, panel_top: 0xff2a418a, panel_bottom: 0xff1d2f66, text: 0xffe8eefc, text_mid: 0xff9fb2e0, accent: 0xff5bb8f0, bg_fill: 0xff09122a, border: 0xff50639f, title_bar: 0xff0b1633, rail: 0xff070e21, footer: 0xff6c7ca6, finish: 0 },
    Faceplate { code: "Cream", name: "Cream", bg_top: 0xffeee6cc, bg_bottom: 0xffd9cfae, panel_top: 0xfff6efd9, panel_bottom: 0xffe4dbbd, text: 0xff23323a, text_mid: 0xff5d7078, accent: 0xff2f98c9, bg_fill: 0xff979079, border: 0xfff7f1df, title_bar: 0xffb8af93, rail: 0xff77715f, footer: 0xff88918a, finish: 0 },
    Faceplate { code: "Silver", name: "Brushed silver", bg_top: 0xffc9cdd0, bg_bottom: 0xffa4a9ad, panel_top: 0xffdfe2e4, panel_bottom: 0xffc3c8cb, text: 0xff1b1f21, text_mid: 0xff4c5458, accent: 0xffc0392b, bg_fill: 0xff727679, border: 0xffe4e7e8, title_bar: 0xff8b8f93, rail: 0xff5a5c5f, footer: 0xff6a7175, finish: 1 },
    Faceplate { code: "Champagne", name: "Champagne gold", bg_top: 0xffd9c68a, bg_bottom: 0xffb89f5a, panel_top: 0xffe6d9a8, panel_bottom: 0xffcdb878, text: 0xff201a0a, text_mid: 0xff574a22, accent: 0xff7a1f1f, bg_fill: 0xff806f3e, border: 0xffeadfb7, title_bar: 0xff9c874c, rail: 0xff655731, footer: 0xff786735, finish: 1 },
    Faceplate { code: "Teal", name: "Vintage teal", bg_top: 0xff2f6b78, bg_bottom: 0xff1b4551, panel_top: 0xff3f8795, panel_bottom: 0xff2c6370, text: 0xffeaf6f8, text_mid: 0xffa6ccd3, accent: 0xffe8c15a, bg_fill: 0xff123038, border: 0xff619ca8, title_bar: 0xff163a44, rail: 0xff0e252c, footer: 0xff759ca5, finish: 0 },
    Faceplate { code: "Dash", name: "Green phosphor dash", bg_top: 0xff04100a, bg_bottom: 0xff010502, panel_top: 0xff0b1d12, panel_bottom: 0xff06120a, text: 0xff7dff9a, text_mid: 0xff3b9c55, accent: 0xff39ff7a, bg_fill: 0xff000301, border: 0xff36453c, title_bar: 0xff000401, rail: 0xff000201, footer: 0xff266737, finish: 3 },
    Faceplate { code: "Amber", name: "Amber LCD", bg_top: 0xff150d02, bg_bottom: 0xff080400, panel_top: 0xff251603, panel_bottom: 0xff170d02, text: 0xffffc34d, text_mid: 0xffa8741a, accent: 0xffff9a1f, bg_fill: 0xff050200, border: 0xff4c3f30, title_bar: 0xff060300, rail: 0xff040200, footer: 0xff704c10, finish: 3 },
    Faceplate { code: "Cyber", name: "Cyber cyan", bg_top: 0xff06141a, bg_bottom: 0xff020609, panel_top: 0xff0b242d, panel_bottom: 0xff07161c, text: 0xff6cf2f8, text_mid: 0xff2f8f98, accent: 0xfff7e03c, bg_fill: 0xff010406, border: 0xff364b52, title_bar: 0xff010507, rail: 0xff010304, footer: 0xff1f5f65, finish: 5 },
    Faceplate { code: "Marine", name: "Dark blue plate", bg_top: 0xff111b30, bg_bottom: 0xff0a1020, panel_top: 0xff26385e, panel_bottom: 0xff1a2848, text: 0xffe9edf5, text_mid: 0xff9db0d0, accent: 0xff6fa5d6, bg_fill: 0xff070b16, border: 0xff4d5b7a, title_bar: 0xff080d1b, rail: 0xff050811, footer: 0xff697892, finish: 0 },
    Faceplate { code: "Granite", name: "Console grey plate", bg_top: 0xff7d8185, bg_bottom: 0xff63676b, panel_top: 0xffb4b8bb, panel_bottom: 0xff9a9ea2, text: 0xff14181a, text_mid: 0xff3c4246, accent: 0xffb03a2e, bg_fill: 0xff45484a, border: 0xffc1c4c7, title_bar: 0xff54575a, rail: 0xff36383a, footer: 0xff494e52, finish: 0 },
    Faceplate { code: "Royal", name: "Royal blue plate", bg_top: 0xff1a2670, bg_bottom: 0xff0f1850, panel_top: 0xff3347b5, panel_bottom: 0xff25339a, text: 0xfff1f3ff, text_mid: 0xffb0b9ee, accent: 0xfff2d21e, bg_fill: 0xff0a1038, border: 0xff5768c2, title_bar: 0xff0c1444, rail: 0xff080d2c, footer: 0xff7780b6, finish: 0 },
    Faceplate { code: "Slate", name: "Rack slate blue-grey", bg_top: 0xff3c4a58, bg_bottom: 0xff28333e, panel_top: 0xff586a7a, panel_bottom: 0xff435261, text: 0xffe6ebef, text_mid: 0xffa5b4c2, accent: 0xff7fc4e8, bg_fill: 0xff1c232b, border: 0xff768491, title_bar: 0xff222b34, rail: 0xff161c22, footer: 0xff798693, finish: 0 },
    Faceplate { code: "Periwinkle", name: "Studio periwinkle", bg_top: 0xff3a3f8a, bg_bottom: 0xff262b6a, panel_top: 0xff5a60bd, panel_bottom: 0xff464ba4, text: 0xfff3f4ff, text_mid: 0xffc2c6f2, accent: 0xffffe27a, bg_fill: 0xff1a1e4a, border: 0xff777cc8, title_bar: 0xff20245a, rail: 0xff14173a, footer: 0xff8b8fc2, finish: 0 },
    Faceplate { code: "Leveler", name: "Pale blue-grey leveler", bg_top: 0xffa9b3b8, bg_bottom: 0xff8a959b, panel_top: 0xffc4ccd0, panel_bottom: 0xffa8b2b8, text: 0xff1c2428, text_mid: 0xff4a565c, accent: 0xff9c2f2f, bg_fill: 0xff60686c, border: 0xffced5d8, title_bar: 0xff757e83, rail: 0xff4b5155, footer: 0xff606c72, finish: 1 },
    Faceplate { code: "Ultramarine", name: "Anodised ultramarine", bg_top: 0xff16246e, bg_bottom: 0xff0c1648, panel_top: 0xff2b43b0, panel_bottom: 0xff1d3190, text: 0xffeef1ff, text_mid: 0xffaebaf0, accent: 0xff41c3ff, bg_fill: 0xff080f32, border: 0xff5164be, title_bar: 0xff0a123d, rail: 0xff060c27, footer: 0xff7580b5, finish: 0 },
    Faceplate { code: "Coal", name: "Module coal black", bg_top: 0xff1f2022, bg_bottom: 0xff0d0d0e, panel_top: 0xff2d2f31, panel_bottom: 0xff1a1b1c, text: 0xffeeeeee, text_mid: 0xff9aa8ae, accent: 0xff4aa8d8, bg_fill: 0xff090909, border: 0xff525456, title_bar: 0xff0b0b0b, rail: 0xff070707, footer: 0xff687176, finish: 1 },
    Faceplate { code: "Steel", name: "Brushed steel", bg_top: 0xff8c9296, bg_bottom: 0xff70777b, panel_top: 0xffa7adb1, panel_bottom: 0xff8d9498, text: 0xff101416, text_mid: 0xff3a4246, accent: 0xffb3342a, bg_fill: 0xff4e5356, border: 0xffb6bbbf, title_bar: 0xff5f6568, rail: 0xff3d4143, footer: 0xff4c5458, finish: 1 },
    Faceplate { code: "Scarlet", name: "Signal scarlet module", bg_top: 0xff6e1f14, bg_bottom: 0xff45120b, panel_top: 0xffb8321f, panel_bottom: 0xff8c2415, text: 0xfff6e6c4, text_mid: 0xffe0b89a, accent: 0xffffd34a, bg_fill: 0xff300c07, border: 0xffc45647, title_bar: 0xff3a0f09, rail: 0xff250906, footer: 0xffa97d67, finish: 0 },
    Faceplate { code: "Cobalt", name: "Rack cobalt", bg_top: 0xff1a2f66, bg_bottom: 0xff10204a, panel_top: 0xff2f58b8, panel_bottom: 0xff224592, text: 0xfff0f4ff, text_mid: 0xffaabdea, accent: 0xffffffff, bg_fill: 0xff0b1633, border: 0xff5476c4, title_bar: 0xff0d1b3e, rail: 0xff081128, footer: 0xff7486b2, finish: 0 },
    Faceplate { code: "Stealth", name: "Stealth black", bg_top: 0xff151618, bg_bottom: 0xff080809, panel_top: 0xff232527, panel_bottom: 0xff151617, text: 0xfff4f4f4, text_mid: 0xff8a8d90, accent: 0xff3a8bff, bg_fill: 0xff050506, border: 0xff4a4c4d, title_bar: 0xff060607, rail: 0xff040404, footer: 0xff5c5e60, finish: 1 },
    Faceplate { code: "Walnut", name: "Walnut cheek wood", bg_top: 0xff4a3322, bg_bottom: 0xff2e1f14, panel_top: 0xff75503a, panel_bottom: 0xff573a28, text: 0xfff3e3c4, text_mid: 0xffcfae86, accent: 0xffe8b15a, bg_fill: 0xff20150e, border: 0xff8d6f5d, title_bar: 0xff271a11, rail: 0xff19110b, footer: 0xff967b5e, finish: 2 },
    Faceplate { code: "Midnight", name: "Midnight blue", bg_top: 0xff101a3c, bg_bottom: 0xff080e24, panel_top: 0xff1c2c62, panel_bottom: 0xff131f48, text: 0xffe3eaff, text_mid: 0xff8fa2d4, accent: 0xffd8d8d8, bg_fill: 0xff050919, border: 0xff44517e, title_bar: 0xff060b1e, rail: 0xff040713, footer: 0xff5f6e96, finish: 0 },
    Faceplate { code: "Azure", name: "Azure plate", bg_top: 0xff1b5a94, bg_bottom: 0xff103e6c, panel_top: 0xff2d7ac2, panel_bottom: 0xff2262a0, text: 0xfff2f8ff, text_mid: 0xffb4d4f0, accent: 0xffffffff, bg_fill: 0xff0b2b4b, border: 0xff5291cc, title_bar: 0xff0d345b, rail: 0xff08223b, footer: 0xff7a9fc1, finish: 0 },
    Faceplate { code: "Oxblood", name: "Oxblood leather", bg_top: 0xff4a1a1a, bg_bottom: 0xff2e0e0e, panel_top: 0xff74302f, panel_bottom: 0xff582222, text: 0xfff1dcc8, text_mid: 0xffc79f88, accent: 0xffe3b07a, bg_fill: 0xff200909, border: 0xff8d5554, title_bar: 0xff270b0b, rail: 0xff190707, footer: 0xff916c5d, finish: 0 },
    Faceplate { code: "Cognac", name: "Cognac tan", bg_top: 0xff9a6a3a, bg_bottom: 0xff744a24, panel_top: 0xffbd8a55, panel_bottom: 0xffa07038, text: 0xff23150a, text_mid: 0xff5b3d1d, accent: 0xff2a4a7a, bg_fill: 0xff513319, border: 0xffc89f73, title_bar: 0xff623e1e, rail: 0xff3f2813, footer: 0xff63411f, finish: 0 },
    Faceplate { code: "Canary", name: "Cyberpunk yellow chassis", bg_top: 0xffc7a812, bg_bottom: 0xff9a8208, panel_top: 0xffe8c820, panel_bottom: 0xffcaa913, text: 0xff14110a, text_mid: 0xff4a420e, accent: 0xff16b4c4, bg_fill: 0xff6b5b05, border: 0xffecd148, title_bar: 0xff826e06, rail: 0xff544704, footer: 0xff66580b, finish: 4 },
    Faceplate { code: "Mesh", name: "Cyberpunk mesh teal", bg_top: 0xff041b1f, bg_bottom: 0xff020b0d, panel_top: 0xff0a3a40, panel_bottom: 0xff062a2f, text: 0xff7ff5e8, text_mid: 0xff32a89c, accent: 0xffff4d6d, bg_fill: 0xff010709, border: 0xff365d62, title_bar: 0xff01090b, rail: 0xff010607, footer: 0xff217169, finish: 5 },
    Faceplate { code: "Redline", name: "Cyberpunk combat red", bg_top: 0xff1a0508, bg_bottom: 0xff0c0204, panel_top: 0xff2c0a10, panel_bottom: 0xff1c060a, text: 0xffff3b4a, text_mid: 0xffa6222e, accent: 0xffff3b4a, bg_fill: 0xff080102, border: 0xff51363b, title_bar: 0xff0a0103, rail: 0xff060102, footer: 0xff70161f, finish: 3 },
    Faceplate { code: "Monolith", name: "Cyberpunk corp black-red", bg_top: 0xff161618, bg_bottom: 0xff08080a, panel_top: 0xff242427, panel_bottom: 0xff151517, text: 0xfff2f2f2, text_mid: 0xffb02a2a, accent: 0xffe5202e, bg_fill: 0xff050507, border: 0xff4b4b4d, title_bar: 0xff060608, rail: 0xff040405, footer: 0xff751e1e, finish: 1 },
    Faceplate { code: "Olive", name: "Cyberpunk olive tactical", bg_top: 0xff2f362a, bg_bottom: 0xff1c2018, panel_top: 0xff474f3f, panel_bottom: 0xff363d30, text: 0xffe0e4cc, text_mid: 0xffa5ae8c, accent: 0xffd9b43a, bg_fill: 0xff131610, border: 0xff686e61, title_bar: 0xff171b14, rail: 0xff0f110d, footer: 0xff757c63, finish: 0 },
    Faceplate { code: "Synthwave", name: "Cyberpunk synthwave neon", bg_top: 0xff24083a, bg_bottom: 0xff12041f, panel_top: 0xff3b1260, panel_bottom: 0xff2a0c47, text: 0xffff7ad9, text_mid: 0xffa66bd8, accent: 0xff35f2ff, bg_fill: 0xff0c0215, border: 0xff5e3c7c, title_bar: 0xff0f031a, rail: 0xff090211, footer: 0xff724697, finish: 5 },
    Faceplate { code: "Optic", name: "Cyberpunk optic blue", bg_top: 0xff061830, bg_bottom: 0xff030b18, panel_top: 0xff0e2c52, panel_bottom: 0xff091f3c, text: 0xff9fe8ff, text_mid: 0xff4a8fb8, accent: 0xffff5ec4, bg_fill: 0xff020710, border: 0xff395171, title_bar: 0xff020914, rail: 0xff01060d, footer: 0xff316080, finish: 5 },
    Faceplate { code: "Ochre", name: "Retro computer ochre", bg_top: 0xffb59a2a, bg_bottom: 0xff8c761a, panel_top: 0xffd2b83c, panel_bottom: 0xffb59a2a, text: 0xff1a1608, text_mid: 0xff4d4313, accent: 0xff1c1c1c, bg_fill: 0xff625212, border: 0xffdac45f, title_bar: 0xff776416, rail: 0xff4d400e, footer: 0xff635415, finish: 0 },
    Faceplate { code: "Rust", name: "Industrial rust", bg_top: 0xff4a3c33, bg_bottom: 0xff2c231d, panel_top: 0xff6b5648, panel_bottom: 0xff4f3e33, text: 0xfff2dcc8, text_mid: 0xffc8a98f, accent: 0xffff7a1a, bg_fill: 0xff1e1814, border: 0xff857468, title_bar: 0xff251d18, rail: 0xff18130f, footer: 0xff917a67, finish: 0 },
    Faceplate { code: "Caution", name: "Caution black-yellow", bg_top: 0xff1a1a1a, bg_bottom: 0xff0a0a0a, panel_top: 0xff2a2a2a, panel_bottom: 0xff181818, text: 0xffffd800, text_mid: 0xffb39600, accent: 0xffffd800, bg_fill: 0xff070707, border: 0xff505050, title_bar: 0xff080808, rail: 0xff050505, footer: 0xff776503, finish: 4 },
    Faceplate { code: "Lime", name: "Lime LCD dash", bg_top: 0xff1c2108, bg_bottom: 0xff0e1104, panel_top: 0xff2d340c, panel_bottom: 0xff1f2408, text: 0xffd7ff3a, text_mid: 0xff88a31f, accent: 0xffd7ff3a, bg_fill: 0xff090b02, border: 0xff525837, title_bar: 0xff0b0e03, rail: 0xff070902, footer: 0xff5d6f15, finish: 3 },
    Faceplate { code: "Neon", name: "Neon glow edge", bg_top: 0xff05080f, bg_bottom: 0xff020307, panel_top: 0xff0b1220, panel_bottom: 0xff060b16, text: 0xff7ff9ff, text_mid: 0xff3a8f9a, accent: 0xffff4fd8, bg_fill: 0xff010204, border: 0xff363c48, title_bar: 0xff010205, rail: 0xff010103, footer: 0xff265e66, finish: 6 },
    Faceplate { code: "Horizon", name: "Synth horizon", bg_top: 0xff1a0733, bg_bottom: 0xff0b031a, panel_top: 0xff2e0f55, panel_bottom: 0xff1e0a3a, text: 0xffffb0f0, text_mid: 0xffa56ad0, accent: 0xff35f2ff, bg_fill: 0xff070212, border: 0xff533a73, title_bar: 0xff090216, rail: 0xff06010e, footer: 0xff6f4590, finish: 7 },
];

pub fn find_faceplate(code: &str) -> Option<&'static Faceplate> {
    FACEPLATES.iter().find(|f| f.code == code)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub parameter: String,
    pub label: String,
    pub style: String,
    pub colour: String,
    pub x: f32, pub y: f32, pub width: f32, pub height: f32, pub font_size: f32,
    pub marks: String,
}
impl Default for Item {
    fn default() -> Self {
        Self { id: String::new(), parameter: String::new(), label: String::new(), style: "metal".into(),
            colour: "#CE9435".into(), x: 0.0, y: 0.0, width: 70.0, height: 76.0, font_size: 10.0, marks: "auto".into() }
    }
}
impl Item {
    pub fn new(id: &str, parameter: &str, label: &str, style: &str, colour: &str, x: f32, y: f32, w: f32, h: f32, font: f32) -> Self {
        Self { id: id.into(), parameter: parameter.into(), label: label.into(), style: style.into(), colour: colour.into(),
            x, y, width: w, height: h, font_size: font, marks: "auto".into() }
    }
}

#[derive(Debug)]
pub struct DesignError(pub String);
impl fmt::Display for DesignError { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "{}", self.0) } }
impl std::error::Error for DesignError {}

pub fn valid_marks(m: &str) -> bool { matches!(m, "auto" | "light" | "dark") }

pub fn validate(i: &Item) -> Result<(), DesignError> {
    if i.id.is_empty() || i.id.contains(['\r','\n']) { return Err(DesignError("Control needs a valid ID".into())); }
    if find_style(&i.style).is_none() { return Err(DesignError(format!("Unknown knob style: {}", i.style))); }
    if i.colour.len()!=7 || !i.colour.starts_with('#') || !i.colour[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(DesignError("Colour must be #RRGGBB".into()));
    }
    if !valid_marks(&i.marks) { return Err(DesignError("Marks must be auto, light or dark".into())); }
    for v in [i.x,i.y,i.width,i.height,i.font_size] {
        if !v.is_finite() { return Err(DesignError("Geometry must be finite".into())); }
    }
    if i.width<20.0 || i.height<20.0 || i.width>4096.0 || i.height>4096.0 || i.font_size<6.0 || i.font_size>72.0 || i.x.abs()>8192.0 || i.y.abs()>8192.0 {
        return Err(DesignError("Geometry is outside supported limits".into()));
    }
    Ok(())
}

pub fn quote(s: &str) -> Result<String, DesignError> {
    if s.contains(['\r','\n']) { return Err(DesignError("Use single-line text".into())); }
    let mut out = String::from("\"");
    for c in s.chars() { if c=='"' { out.push('"'); } out.push(c); }
    out.push('"'); Ok(out)
}
fn quote_if_needed(s: &str) -> Result<String, DesignError> {
    let plain = !s.contains([',','"']) && (s.is_empty() || (!s.starts_with(' ') && !s.ends_with(' '))) && !s.contains(['\r','\n']);
    if plain { Ok(s.to_string()) } else { quote(s) }
}
pub fn fields(line: &str) -> Result<Vec<String>, DesignError> {
    let mut out=Vec::new(); let mut value=String::new(); let mut quoted=false; let mut closed=false;
    let b=line.as_bytes(); let mut n=0;
    while n<b.len() {
        let c=b[n] as char;
        if quoted {
            if c=='"' {
                if n+1<b.len() && b[n+1]==b'"' { value.push('"'); n+=1; }
                else { quoted=false; closed=true; }
            } else { value.push(c); }
        } else if c==',' { out.push(std::mem::take(&mut value)); closed=false; }
        else if c=='"' && value.is_empty() && !closed { quoted=true; }
        else {
            if closed || c=='"' { return Err(DesignError("Malformed CSV quoting".into())); }
            value.push(c);
        }
        n+=1;
    }
    if quoted { return Err(DesignError("Unclosed CSV quote".into())); }
    out.push(value); Ok(out)
}

pub const HEADER_V1: &str = "id,parameter,label,style,colour,x,y,width,height,font size";
pub const HEADER: &str = "id,parameter,label,style,colour,x,y,width,height,font size,marks";

fn fmt_num(v: f32) -> String {
    if v.is_finite() && v.fract()==0.0 && v.abs() < 1.0e7 { format!("{}", v as i64) }
    else {
        // max_digits10 for f32 is 9 significant digits, defaultfloat.
        let s = format!("{:.8}", v);
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() || s=="-" { "0".into() } else { s }
    }
}

pub fn write_design(items: &[Item]) -> Result<String, DesignError> {
    let any_marks = items.iter().any(|i| i.marks != "auto");
    let mut out = String::from("GoodLookinUI design version 1\n");
    out.push_str(if any_marks { HEADER } else { HEADER_V1 });
    out.push('\n');
    let mut ids = BTreeSet::new();
    for i in items {
        validate(i)?;
        if !ids.insert(i.id.clone()) { return Err(DesignError("Duplicate ID".into())); }
        out.push_str(&quote_if_needed(&i.id)?); out.push(',');
        out.push_str(&quote_if_needed(&i.parameter)?); out.push(',');
        out.push_str(&quote_if_needed(&i.label)?); out.push(',');
        out.push_str(&quote_if_needed(&i.style)?); out.push(',');
        out.push_str(&quote_if_needed(&i.colour)?); out.push(',');
        out.push_str(&fmt_num(i.x)); out.push(',');
        out.push_str(&fmt_num(i.y)); out.push(',');
        out.push_str(&fmt_num(i.width)); out.push(',');
        out.push_str(&fmt_num(i.height)); out.push(',');
        out.push_str(&fmt_num(i.font_size));
        if any_marks { out.push(','); out.push_str(&quote_if_needed(&i.marks)?); }
        out.push('\n');
    }
    Ok(out)
}

fn parse_num(s: &str) -> Result<f32, DesignError> {
    if s.is_empty() || s.contains(char::is_whitespace) { return Err(DesignError("Invalid number".into())); }
    let v: f32 = s.parse().map_err(|_| DesignError("Invalid number".into()))?;
    if !v.is_finite() { return Err(DesignError("Invalid number".into())); }
    // reject trailing junk: f32 parse already consumes all or fails
    Ok(v)
}

pub fn read_design(text: &str) -> Result<Vec<Item>, DesignError> {
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let head = lines.next().unwrap_or("");
    if head != "GoodLookinUI design version 1" && head != "HardwareUI design version 1" {
        return Err(DesignError("Unsupported design version".into()));
    }
    let cols = lines.next().unwrap_or("");
    let has_marks = cols == HEADER;
    if !has_marks && cols != HEADER_V1 { return Err(DesignError("Invalid design columns".into())); }
    let expect = if has_marks { 11 } else { 10 };
    let mut result = Vec::new();
    let mut ids = BTreeSet::new();
    for line in lines {
        if line.is_empty() { continue; }
        let f = fields(line)?;
        if f.len() != expect { return Err(DesignError(if has_marks { "Expected eleven columns" } else { "Expected ten columns" }.into())); }
        let item = Item {
            id: f[0].clone(), parameter: f[1].clone(), label: f[2].clone(), style: f[3].clone(), colour: f[4].clone(),
            x: parse_num(&f[5])?, y: parse_num(&f[6])?, width: parse_num(&f[7])?, height: parse_num(&f[8])?, font_size: parse_num(&f[9])?,
            marks: if has_marks { f[10].clone() } else { "auto".into() },
        };
        validate(&item)?;
        if !ids.insert(item.id.clone()) { return Err(DesignError("Duplicate ID".into())); }
        result.push(item);
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Motion { pub position: f64, pub velocity: f64 }
impl Motion {
    pub fn step(&mut self, target: f64, seconds: f64, frequency: f64) {
        let seconds = seconds.clamp(0.0, 0.1);
        let offset = self.position - target;
        let decay = (-frequency * seconds).exp();
        let term = self.velocity + frequency * offset;
        self.position = target + (offset + term * seconds) * decay;
        self.velocity = (self.velocity - frequency * term * seconds) * decay;
    }
}
