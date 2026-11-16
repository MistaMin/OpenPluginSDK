//! From a manifest to Rust: a `&[ParamInfo]` table, a `PluginInfo`, and the
//! identity pins that make the port the same plugin to a host
//! (`openplugin_core::identity`): the parameter ids of the scanned format, the
//! VST3 class ids, the Audio Unit codes and the AAX ids. Every guess is
//! written next to the item it concerns, and what a person must still decide
//! is marked `TODO`.

use std::fmt::Write;

use openplugin_core::identity::{PinFormat, VST3_BYPASS_PARAM_ID, VST3_MAX_PARAM_ID, VST3_MIDI_PARAM_BASE, VST3_MIDI_PARAM_COUNT};

use crate::manifest::*;

// ---------- identity pins ----------

/// What the table says about one parameter's id in the scanned format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    /// The source's id is the parameter's position, as the SDK numbers an
    /// unpinned table: nothing to write.
    Position,
    /// Keep the source's id with this `ParamInfo` method (`with_clap_id`,
    /// `with_vst3_id` or `with_au_id`).
    Id { method: &'static str, id: u32 },
    /// The id cannot be kept, and why.
    Unpinnable(String),
}

/// The numeric-id format a manifest was scanned from, if it has one.
fn pin_format(format: &str) -> Option<(PinFormat, &'static str)> {
    match format {
        "clap" => Some((PinFormat::Clap, "with_clap_id")),
        "vst3" => Some((PinFormat::Vst3, "with_vst3_id")),
        "au" => Some((PinFormat::Au, "with_au_id")),
        _ => None,
    }
}

fn unpinnable(format: PinFormat, id: u32) -> Option<String> {
    match format {
        PinFormat::Clap if id == u32::MAX => Some("CLAP_INVALID_ID cannot be a parameter id".into()),
        PinFormat::Au if id == u32::MAX => Some("the SDK keeps 0xFFFFFFFF for \"no parameter\"".into()),
        PinFormat::Vst3 if id == VST3_BYPASS_PARAM_ID => {
            Some(format!("{id} is the id of the bypass parameter the SDK's VST3 shell adds itself: leave this parameter out unless the plugin has a bypass of its own"))
        }
        PinFormat::Vst3 if id >= VST3_MIDI_PARAM_BASE && id - VST3_MIDI_PARAM_BASE < VST3_MIDI_PARAM_COUNT => {
            Some(format!("{id} is in the range the SDK's VST3 shell uses for MIDI controllers"))
        }
        PinFormat::Vst3 if id > VST3_MAX_PARAM_ID => Some(format!("{id} is above the largest VST3 ParamID a plugin may use")),
        _ => None,
    }
}

/// One [`Pin`] per parameter of a manifest scanned from `format` (`clap`,
/// `vst3`, `au`). A table whose ids are all positions needs no pins. As soon
/// as one parameter needs one, every parameter gets its pin, so the table can
/// be reordered later without changing any id.
pub fn pins(format: &str, params: &[Param]) -> Vec<Pin> {
    let Some((kind, method)) = pin_format(format) else {
        return vec![Pin::Position; params.len()];
    };
    let blocked: Vec<Option<String>> = params.iter().map(|p| unpinnable(kind, p.id)).collect();
    let needed = params.iter().zip(&blocked).enumerate().any(|(index, (p, why))| why.is_none() && p.id as usize != index);
    params
        .iter()
        .zip(blocked)
        .enumerate()
        .map(|(index, (p, why))| match why {
            Some(why) => Pin::Unpinnable(why),
            None if params[..index].iter().any(|earlier| earlier.id == p.id) => Pin::Unpinnable(format!("another parameter of the source already has id {}", p.id)),
            None if needed => Pin::Id { method, id: p.id },
            None => Pin::Position,
        })
        .collect()
}

/// What the scanner decided one parameter is, in the SDK's terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Inferred {
    /// The string id the SDK addresses the parameter by.
    pub id: String,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// A `Unit` variant name: `Db`, `Hz`, `Toggle`, `Choice`, `None`...
    pub unit: &'static str,
    /// `Linear` or `Log`.
    pub taper: &'static str,
    /// `with_steps` argument; 0 when the SDK's unit already implies the steps.
    pub steps: u32,
    pub choices: Vec<String>,
    /// `ParamFlags` constants the parameter carries, `AUTOMATABLE` included when it is.
    pub flags: Vec<&'static str>,
    /// What was guessed, and from what.
    pub notes: Vec<String>,
}

// ---------- parsing value strings ----------

/// A number and the unit word after it, as printed by a plugin: `-12.5 dB`,
/// `1.5 kHz`, `50%`, `3:1`. `kHz` is scaled to Hz. `-inf` and text without a
/// leading number give `None`.
pub fn parse_value(text: &str) -> Option<(f64, String)> {
    let t = text.trim();
    let bytes = t.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    let mut value: f64 = t[..i].parse().ok()?;
    let rest = t[i..].trim();
    // A ratio such as 4:1 reads as 4.
    let unit = rest.split_whitespace().next().unwrap_or("").to_string();
    if unit.eq_ignore_ascii_case("khz") {
        value *= 1000.0;
        return Some((value, "Hz".into()));
    }
    Some((value, unit))
}

fn unit_variant(word: &str) -> Option<&'static str> {
    match word.trim().to_ascii_lowercase().as_str() {
        "db" => Some("Db"),
        "hz" | "khz" => Some("Hz"),
        "ms" => Some("Ms"),
        "s" | "sec" | "secs" => Some("Seconds"),
        "%" | "percent" => Some("Percent"),
        ":1" | "x" => Some("Ratio"),
        "st" | "semitones" | "semitone" => Some("Semitones"),
        "ct" | "cents" | "cent" => Some("Cents"),
        _ => None,
    }
}

fn is_toggle_text(t: &str) -> bool {
    matches!(
        t.trim().to_ascii_lowercase().as_str(),
        "" | "off" | "on" | "0" | "1" | "no" | "yes" | "false" | "true" | "bypass" | "bypassed" | "active" | "enabled" | "disabled" | "0.00" | "1.00" | "0.0" | "1.0"
    )
}

// ---------- fitting a taper ----------

/// How well a straight line and an exponential curve (`y0 * (y1/y0)^t`)
/// through the end points match the middle points. Errors are relative to the
/// travel of the curve: 0 is exact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub linear_error: f64,
    pub log_error: Option<f64>,
}

pub fn fit(points: &[(f64, f64)]) -> Option<Fit> {
    let (first, last) = (points.first()?, points.last()?);
    let (y0, y1) = (first.1, last.1);
    if !y0.is_finite() || !y1.is_finite() || points.iter().any(|p| !p.1.is_finite()) {
        return None;
    }
    let span = y1 - y0;
    if span.abs() < 1e-12 {
        return None;
    }
    let linear_error = points.iter().map(|&(t, y)| (y - (y0 + span * t)).abs() / span.abs()).fold(0.0, f64::max);
    let log_error = if y0 > 0.0 && y1 > 0.0 && points.iter().all(|p| p.1 > 0.0) {
        let l = (y1 / y0).ln();
        Some(points.iter().map(|&(t, y)| ((y / y0).ln() - l * t).abs() / l.abs()).fold(0.0, f64::max))
    } else {
        None
    };
    Some(Fit { linear_error, log_error })
}

/// True when the exponential curve explains the points and a line clearly does not.
pub fn is_log(f: &Fit) -> bool {
    f.log_error.is_some_and(|e| e <= 0.03) && f.linear_error >= 0.08
}

// ---------- inference ----------

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// One `Inferred` per parameter, with unique string ids.
pub fn infer(plugin: &Plugin) -> Vec<Inferred> {
    let mut used: Vec<String> = Vec::new();
    plugin
        .params
        .iter()
        .map(|p| {
            let mut inferred = infer_param(p);
            let base = {
                let s = slug(&p.name);
                if s.is_empty() {
                    format!("param_{}", p.id)
                } else if s.as_bytes()[0].is_ascii_digit() {
                    format!("p_{s}")
                } else {
                    s
                }
            };
            let mut id = base.clone();
            let mut n = 2;
            while used.contains(&id) {
                id = format!("{base}_{n}");
                n += 1;
            }
            if id != base {
                inferred.notes.push(format!("id made unique: another parameter is also called \"{}\"", p.name));
            }
            used.push(id.clone());
            inferred.id = id;
            inferred
        })
        .collect()
}

fn infer_param(p: &Param) -> Inferred {
    let mut notes = Vec::new();
    let texts: Vec<&str> = p.samples.iter().map(|s| s.text.as_str()).collect();
    let parsed: Vec<Option<(f64, String)>> = texts.iter().map(|t| parse_value(t)).collect();
    let mut numeric = !parsed.is_empty() && parsed.iter().all(Option::is_some);
    // A plugin with no display function of its own prints the normalised value
    // (0.25 at 25%) even when its plain range is something else: the text then
    // says nothing about the range or the curve.
    let text_is_position = numeric
        && p.samples.iter().zip(&parsed).all(|(s, v)| v.as_ref().is_some_and(|(y, _)| (y - s.normalized).abs() < 1e-3))
        && p.samples.iter().any(|s| (s.plain - s.normalized).abs() > 1e-3);
    if text_is_position {
        numeric = false;
    }

    // Unit: the format's own text first, then the word after the number.
    let mut unit = unit_variant(&p.units);
    if unit.is_none() {
        unit = parsed.iter().flatten().find_map(|(_, u)| unit_variant(u));
    }

    let mut flags: Vec<&'static str> = Vec::new();
    let automatable = p.has_flag("automatable") && !p.has_flag("read_only");
    if automatable {
        flags.push("AUTOMATABLE");
    }
    if p.has_flag("read_only") {
        flags.push("READ_ONLY");
    }
    if p.has_flag("hidden") {
        flags.push("HIDDEN");
    }
    if p.has_flag("bypass") {
        flags.push("BYPASS");
    }
    if p.has_flag("non_realtime") {
        flags.push("NOT_REALTIME");
    }
    if !p.has_flag("automatable") && !p.has_flag("read_only") && !p.has_flag("non_realtime") {
        notes.push("not automatable in the source; the SDK has no flag for that alone (it keeps AUTOMATABLE off through READ_ONLY or NOT_REALTIME), so NOT_REALTIME is used".into());
        flags.push("NOT_REALTIME");
    }

    let mut out = Inferred {
        id: String::new(),
        name: p.name.clone(),
        min: p.min as f32,
        max: p.max as f32,
        default: p.default as f32,
        unit: unit.unwrap_or("None"),
        taper: "Linear",
        steps: 0,
        choices: Vec::new(),
        flags,
        notes,
    };

    // Discrete parameters: toggle, choice list, or stepped number.
    if p.step_count >= 1 {
        let default_index = (p.default_normalized * f64::from(p.step_count)).round();
        let nonnumeric = p.step_texts.iter().any(|t| parse_value(t).is_none() && !t.trim().is_empty());
        if p.step_count == 1 && p.step_texts.iter().all(|t| is_toggle_text(t)) {
            out.unit = "Toggle";
            out.min = 0.0;
            out.max = 1.0;
            out.default = default_index as f32;
            return out;
        }
        if nonnumeric && p.step_texts.len() == p.step_count as usize + 1 {
            out.unit = "Choice";
            out.min = 0.0;
            out.max = p.step_count as f32;
            out.default = default_index as f32;
            out.choices = p.step_texts.clone();
            return out;
        }
        out.steps = p.step_count + 1;
    }

    // The range and curve of a continuous (or numerically stepped) parameter.
    let plain_points: Vec<(f64, f64)> = p.samples.iter().map(|s| (s.normalized, s.plain)).collect();
    let plain_fit = fit(&plain_points);
    let text_points: Vec<(f64, f64)> = if numeric {
        p.samples.iter().zip(&parsed).filter_map(|(s, v)| v.as_ref().map(|(y, _)| (s.normalized, *y))).collect()
    } else {
        Vec::new()
    };
    let text_fit = if text_points.len() == p.samples.len() { fit(&text_points) } else { None };

    let travel = (p.max - p.min).abs();
    let flagged_log = p.has_flag("log_display") && p.min > 0.0;
    if let Some(f) = plain_fit.filter(is_log) {
        out.taper = "Log";
        out.notes.push(format!("taper Log: the plain values at 0, 25, 50, 75 and 100% follow an exponential curve (error {:.3} against {:.3} for a line)", f.log_error.unwrap_or(0.0), f.linear_error));
    } else if flagged_log {
        out.taper = "Log";
        out.notes.push("taper Log: the source marks the parameter as logarithmic".into());
    } else if let (Some(f), true) = (text_fit, text_points.len() == p.samples.len()) {
        let (t0, t1) = (text_points[0].1, text_points[text_points.len() - 1].1);
        let differs = (t0 - p.min).abs() > 0.002 * travel + 1e-6 || (t1 - p.max).abs() > 0.002 * travel + 1e-6;
        if t1 > t0 && (is_log(&f) || differs) {
            if differs {
                // The text is in other units than the plain range: take the text's.
                out.min = t0 as f32;
                out.max = t1 as f32;
                let dn = p.default_normalized;
                let curve = if is_log(&f) { t0 * (t1 / t0).powf(dn) } else { t0 + (t1 - t0) * dn };
                out.default = curve as f32;
                out.notes.push(format!("range {t0}..{t1} read from the value strings; the plugin's plain range is {}..{}; default placed on the same curve", p.min, p.max));
            }
            if is_log(&f) && t0 > 0.0 {
                out.taper = "Log";
                out.notes.push(format!("taper Log: the value strings ({}) follow an exponential curve (error {:.3} against {:.3} for a line)", texts.join(", "), f.log_error.unwrap_or(0.0), f.linear_error));
            } else {
                out.notes.push(format!("taper Linear: the value strings ({}) fit a line (error {:.3})", texts.join(", "), f.linear_error));
            }
        } else {
            out.notes.push(format!("taper Linear: the value strings ({}) fit a line (error {:.3})", texts.join(", "), f.linear_error));
        }
    } else if text_is_position {
        out.notes.push("taper Linear (guessed): the value strings are the normalised position (the plugin has no display function), so they say nothing about the curve".into());
    } else if numeric {
        out.notes.push("taper Linear (guessed): the value strings are not usable for a fit (flat, or not finite at an end)".into());
    } else {
        out.notes.push("taper Linear (guessed): the plugin gave no numeric value strings to fit".into());
    }
    if out.unit == "None" && unit.is_none() && numeric {
        out.notes.push("unit None: no unit word in the units text or the value strings".into());
    }
    if out.min >= out.max {
        out.notes.push("the range is empty or reversed in the source; check it by hand".into());
    }
    out
}

// ---------- rendering ----------

/// A parameter id as a Rust literal: small numbers in decimal, the rest in hex
/// (ids made from a hash or from four characters read better that way).
fn id_literal(id: u32) -> String {
    if id < 10_000 { id.to_string() } else { format!("{id:#010X}") }
}

/// The name a format goes by in the generated comments.
fn format_name(format: &str) -> &str {
    match format {
        "clap" => "CLAP",
        "vst3" => "VST3",
        "au" => "Audio Unit",
        "aax" => "AAX",
        other => other,
    }
}

/// An `f32` literal with six significant digits at most (`0.6000001` reads as `0.6`).
fn lit(v: f32) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    if v == 0.0 {
        return "0.0".into();
    }
    let magnitude = f64::from(v).abs().log10().floor() as i32;
    let digits = (5 - magnitude).clamp(1, 12) as usize;
    let mut text = format!("{:.*}", digits, f64::from(v));
    while text.ends_with('0') && !text.ends_with(".0") {
        text.pop();
    }
    // A very small value that rounded to zero keeps the exact form instead.
    if text.trim_start_matches('-') == "0.0" { format!("{v:e}") } else { text }
}

fn category(plugin: &Plugin) -> &'static str {
    let cats: Vec<String> = plugin.categories.iter().map(|c| c.to_ascii_lowercase()).collect();
    let has = |words: &[&str]| cats.iter().any(|c| words.iter().any(|w| c.contains(w)));
    if plugin.is_instrument || has(&["instrument", "synth", "sampler"]) {
        "Instrument"
    } else if has(&["eq"]) {
        "Eq"
    } else if has(&["dynamics", "compressor", "limiter", "gate", "expander", "de-esser"]) {
        "Dynamics"
    } else if has(&["pitch"]) {
        "PitchShift"
    } else if has(&["reverb"]) {
        "Reverb"
    } else if has(&["delay", "echo"]) {
        "Delay"
    } else if has(&["modulation", "chorus", "flanger", "phaser", "tremolo"]) {
        "Modulation"
    } else if has(&["distortion", "saturat", "overdrive"]) {
        "Harmonic"
    } else if has(&["dither"]) {
        "Dither"
    } else if has(&["tools", "utility", "analyzer", "analyser", "mastering"]) {
        "Utility"
    } else {
        "Other"
    }
}

fn layout_name(channels: u32) -> Option<&'static str> {
    Some(match channels {
        0 => "SpeakerLayout::Absent",
        1 => "SpeakerLayout::Mono",
        2 => "SpeakerLayout::Stereo",
        _ => return None,
    })
}

/// The Rust text for a manifest. `file_note` names the source in the header.
pub fn render(m: &Manifest) -> String {
    let p = &m.plugin;
    let inferred = infer(p);
    let mut s = String::new();
    let _ = writeln!(s, "// Generated by {} from {} ({} plugin {} of {}).", m.tool, m.source.path, m.source.format, m.source.index + 1, m.source.count);
    let _ = writeln!(s, "// Review before use: every line marked \"guessed\" or TODO needs a person.");
    let _ = writeln!(s, "use openplugin::prelude::*;");
    let _ = writeln!(s);
    for w in &p.warnings {
        let _ = writeln!(s, "// scan warning: {w}");
    }
    if !p.warnings.is_empty() {
        let _ = writeln!(s);
    }

    // ----- parameters -----
    let _ = writeln!(s, "// ---- parameters ----");
    let _ = writeln!(s, "// The SDK addresses a parameter by string id and by its position in this table.");
    let _ = writeln!(s, "// The string ids below are made from the names; the source's own ids are in each comment.");
    let param_pins = pins(&m.source.format, &p.params);
    let pinned = param_pins.iter().any(|pin| matches!(pin, Pin::Id { .. }));
    if pinned {
        let _ = writeln!(s, "// The source's {} parameter ids are not positions, so each parameter is pinned to its id", format_name(&m.source.format));
        let _ = writeln!(s, "// (openplugin::core::identity): automation and saved projects find it wherever it is in the table.");
        let _ = writeln!(s, "// The pins are for {} alone. Scan the plugin's other formats too: where a parameter has the same", format_name(&m.source.format));
        let _ = writeln!(s, "// number in CLAP, VST3 and Audio Unit, write with_host_id(n) instead; AAX takes with_aax_id(\"...\").");
        let _ = writeln!(s, "// Assert check_pins(&plugin) == [] in a test.");
    }
    let _ = writeln!(s, "pub static PARAMS: [ParamInfo; {}] = [", p.params.len());
    for ((src, inf), pin) in p.params.iter().zip(&inferred).zip(&param_pins) {
        let _ = writeln!(s, "    // source id {}: \"{}\"{}", src.id, src.name, src.group.as_ref().map(|g| format!(", group {g}")).unwrap_or_default());
        for n in &inf.notes {
            let _ = writeln!(s, "    // {n}");
        }
        if let Pin::Unpinnable(why) = pin {
            let _ = writeln!(s, "    // TODO(porting): the id cannot be pinned: {why}");
        }
        let mut line = format!("    ParamInfo::new({:?}, {:?}, {}, {}, {}, Unit::{})", inf.id, inf.name, lit(inf.min), lit(inf.max), lit(inf.default), inf.unit);
        if inf.taper == "Log" {
            line.push_str(".with_taper(Taper::Log)");
        }
        if inf.unit == "Choice" {
            let list: Vec<String> = inf.choices.iter().map(|c| format!("{c:?}")).collect();
            let _ = write!(line, ".with_choices(&[{}])", list.join(", "));
        }
        if inf.steps > 1 {
            let _ = write!(line, ".with_steps({})", inf.steps);
        }
        for f in inf.flags.iter().filter(|f| **f != "AUTOMATABLE") {
            let _ = write!(line, ".flag(ParamFlags::{f})");
        }
        if let Pin::Id { method, id } = pin {
            let _ = write!(line, ".{method}({})", id_literal(*id));
        }
        line.push(',');
        let _ = writeln!(s, "{line}");
    }
    let _ = writeln!(s, "];");
    let _ = writeln!(s);

    // ----- plugin info -----
    let ids = &p.ids;
    let (id_text, id_note) = match &ids.clap {
        Some(c) => (c.id.clone(), None),
        None => {
            let v = slug(&p.vendor).replace('_', "");
            let n = slug(&p.name).replace('_', "-");
            (format!("com.{}.{}", if v.is_empty() { "vendor" } else { &v }, n), Some("guessed: the source has no CLAP id, so this is made from the vendor and name; PluginInfo::id becomes the CLAP id and drives the VST3 class ids"))
        }
    };
    let _ = writeln!(s, "// ---- identity ----");
    if let Some(n) = id_note {
        let _ = writeln!(s, "// {n}");
    }
    let _ = writeln!(s, "pub static INFO: PluginInfo = PluginInfo {{");
    let _ = writeln!(s, "    id: {id_text:?},");
    let _ = writeln!(s, "    name: {:?},", p.name);
    let _ = writeln!(s, "    vendor: {:?},", p.vendor);
    let _ = writeln!(s, "    url: {:?},", p.url);
    let _ = writeln!(s, "    email: {:?},", p.email);
    let _ = writeln!(s, "    version: {:?},", p.version);
    let _ = writeln!(s, "    category: Category::{},", category(p));
    let _ = writeln!(s, "    params: &PARAMS,");
    let _ = writeln!(s, "    is_instrument: {},", p.is_instrument);
    let _ = writeln!(s, "    has_editor: false,");
    let _ = writeln!(s, "}};");
    let _ = writeln!(s);

    // ----- build settings -----
    let _ = writeln!(s, "// ---- build settings (scripts/build_plugin.sh, docs/BUILD.md) ----");
    let mut args = format!("--name {:?}", p.name);
    if !p.vendor.is_empty() {
        let _ = write!(args, " --manufacturer {:?}", p.vendor);
    }
    let version_numbers: String = p.version.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if !version_numbers.is_empty() {
        let _ = write!(args, " --version {version_numbers}");
    }
    if let Some(b) = &m.source.bundle_id {
        // The shells add their own suffix (.vst3, .au, .aax) to the prefix.
        let prefix = [".vst3", ".au", ".aax"].iter().find_map(|x| b.strip_suffix(x)).unwrap_or(b);
        let _ = write!(args, " --bundle-id {prefix:?}");
    }
    if let Some(au) = &ids.au {
        let codes = au.subtype.len() == 4 && au.manufacturer.len() == 4 && au.subtype.is_ascii() && au.manufacturer.is_ascii();
        if codes {
            let _ = writeln!(s, "// Audio Unit codes, pinned in the Plugin impl (the shell build reads them; --au-subtype and");
            let _ = writeln!(s, "// --au-manufacturer are then left out, or must say the same):");
            let _ = writeln!(s, "//   fn au_ids(&self) -> Option<AuIds> {{");
            let _ = writeln!(s, "//       Some(AuIds {{ subtype: *b{:?}, manufacturer: *b{:?} }})", au.subtype, au.manufacturer);
            let _ = writeln!(s, "//   }}");
        } else {
            let _ = write!(args, " --au-subtype {} --au-manufacturer {}", au.subtype, au.manufacturer);
            let _ = writeln!(s, "// TODO(porting): the Audio Unit codes '{}' and '{}' are not four ASCII characters each; pass them to the build by hand.", au.subtype, au.manufacturer);
        }
        if matches!(au.component_type.as_str(), "aufx" | "aumu" | "aumf") {
            // The shell picks aufx or aumu from the plugin's kind; aumf is asked for.
            if au.component_type == "aumf" {
                let _ = write!(args, " --au-type aumf");
                let _ = writeln!(s, "// The source is a music effect ('aumf'): build with --au-type aumf (-DOP_AU_TYPE=aumf).");
            } else {
                let _ = writeln!(s, "// Audio Unit type '{}': the shell picks it from the plugin's kind (--au-type auto).", au.component_type);
            }
        } else {
            let _ = writeln!(s, "// TODO(porting): the source is a '{}' component; the Audio Unit shell builds aufx, aumu and aumf only.", au.component_type);
        }
    } else {
        let _ = writeln!(s, "// No Audio Unit codes in the source: pick a four-character subtype and manufacturer (Plugin::au_ids, or --au-subtype and --au-manufacturer).");
        let _ = writeln!(s, "// TODO(porting): an existing Audio Unit's codes must be kept when the plugin is also shipped as one; scan the .component.");
    }
    let _ = writeln!(s, "// scripts/build_plugin.sh {args} ...");
    if let Some(aax) = &ids.aax {
        let plug: Vec<String> = aax.plugin_ids.iter().map(|x| format!("{x:?}")).collect();
        let _ = writeln!(s, "// AAX ids (found in the {}): manufacturer {:?}, product {:?}, plug-in types [{}]", aax.found_in, aax.manufacturer, aax.product, plug.join(", "));
        if aax.plugin_ids.len() >= 2 && aax.manufacturer.len() == 4 && aax.product.len() == 4 && aax.plugin_ids.iter().take(2).all(|x| x.len() == 4) {
            let _ = writeln!(s, "// In the Plugin impl:");
            let _ = writeln!(s, "//   fn aax_ids(&self) -> Option<AaxIds> {{");
            let _ = writeln!(s, "//       Some(AaxIds {{ manufacturer: *b{:?}, product: *b{:?}, mono: *b{:?}, stereo: *b{:?} }})", aax.manufacturer, aax.product, aax.plugin_ids[0], aax.plugin_ids[1]);
            let _ = writeln!(s, "//   }}");
            let _ = writeln!(s, "// (the first two plug-in types are taken as mono and stereo; check the page table)");
        }
    } else if m.source.format == "aax" {
        let _ = writeln!(s, "// TODO(porting): no AAX ids could be read from the bundle.");
    }

    // ----- class ids and parameter ids -----
    let _ = writeln!(s);
    let _ = writeln!(s, "// ---- identity pins (openplugin::core::identity) ----");
    if let Some(v) = &ids.vst3 {
        let _ = writeln!(s, "// VST3 class ids, in the Plugin impl: a host finds the plugin in a project by the processor's.");
        let _ = writeln!(s, "//   fn vst3_ids(&self) -> Option<Vst3Ids> {{");
        match &v.controller_class_id {
            Some(c) => {
                let _ = writeln!(s, "//       Vst3Ids::from_hex({:?}, {c:?})", v.class_id);
                let _ = writeln!(s, "//   }}");
            }
            None => {
                let _ = writeln!(s, "//       Vst3Ids::processor_from_hex({:?})", v.class_id);
                let _ = writeln!(s, "//   }}");
                let _ = writeln!(s, "// The source is a single-component VST3 (no separate controller class); the SDK always writes two classes,");
                let _ = writeln!(s, "// and the controller's id stays derived from PluginInfo::id.");
            }
        }
    }
    if ids.clap.is_some() && m.source.format != "clap" {
        let _ = writeln!(s, "// The CLAP id is PluginInfo::id above.");
    }
    let unpinnable = param_pins.iter().filter(|pin| matches!(pin, Pin::Unpinnable(_))).count();
    if p.params.is_empty() {
    } else if pinned {
        let _ = writeln!(s, "// Parameter ids: pinned in the table above with the source's {} ids.", format_name(&m.source.format));
    } else if pin_format(&m.source.format).is_some() {
        let _ = writeln!(s, "// Parameter ids in the source are positions in table order, as in the SDK: nothing to pin.");
    } else {
        let _ = writeln!(s, "// Parameter ids: a scan of the {} bundle reads none; scan the plugin's CLAP, VST3 or Audio Unit for them.", format_name(&m.source.format));
    }
    if unpinnable > 0 {
        let _ = writeln!(s, "// TODO(porting): {unpinnable} parameter id(s) could not be pinned; see the table above.");
    }

    // ----- audio and timing -----
    let _ = writeln!(s);
    let _ = writeln!(s, "// ---- audio ----");
    let main_in = p.buses.iter().find(|b| b.direction == "input" && b.kind == "main");
    let main_out = p.buses.iter().find(|b| b.direction == "output" && b.kind == "main");
    let side = p.buses.iter().find(|b| b.direction == "input" && b.kind == "aux");
    for b in &p.buses {
        let _ = writeln!(s, "// bus: {} {} \"{}\", {} channels{}", b.direction, b.kind, b.name, b.channels, b.arrangement.as_ref().map(|a| format!(", arrangement {a}")).unwrap_or_default());
    }
    for l in &p.channel_layouts {
        let _ = writeln!(s, "// supported channel layout: {} in, {} out", l.inputs, l.outputs);
    }
    let in_ch = main_in.map_or(0, |b| b.channels);
    let out_ch = main_out.map_or(0, |b| b.channels);
    if let (Some(i), Some(o)) = (layout_name(in_ch), layout_name(out_ch)) {
        let sc = match side.map(|b| b.channels) {
            Some(c) => layout_name(c).map(|l| format!("Some({l})")),
            None => Some("None".to_string()),
        };
        match sc {
            Some(sc) => {
                let _ = writeln!(s, "// In the Plugin impl:");
                let _ = writeln!(s, "//   fn supported_layouts(&self) -> &'static [IoConfig] {{");
                let _ = writeln!(s, "//       &[IoConfig {{ input: {i}, output: {o}, sidechain: {sc} }}]");
                let _ = writeln!(s, "//   }}");
            }
            None => {
                let _ = writeln!(s, "// TODO(porting): the sidechain has an unusual width; write supported_layouts by hand.");
            }
        }
    } else if !p.buses.is_empty() {
        let _ = writeln!(s, "// TODO(porting): the main buses are {in_ch} in and {out_ch} out; write supported_layouts by hand (see crates/openplugin-core/src/layout.rs).");
    }
    match p.latency_samples {
        Some(n) => {
            let _ = writeln!(s, "// latency: {n} samples (Plugin::latency)");
        }
        None => {
            let _ = writeln!(s, "// latency: not reported");
        }
    }
    match (p.tail_samples, p.tail_seconds) {
        (Some(u32::MAX), _) => {
            let _ = writeln!(s, "// tail: endless (Plugin::tail -> Tail::Infinite)");
        }
        (Some(n), _) => {
            let _ = writeln!(s, "// tail: {n} samples (Plugin::tail -> Tail::Finite({n}))");
        }
        (None, _) => {
            let _ = writeln!(s, "// tail: not reported");
        }
    }
    s
}
