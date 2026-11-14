//! Factory presets as plain text, and a bank that serves them to every format
//! through [`crate::Plugin::preset_count`], [`crate::Plugin::preset_name`]
//! and [`crate::Plugin::preset_values`].
//!
//! ```text
//! # A comment. Blank lines are fine.
//! name = Recital Hall
//! mode = Hall
//! reverb_time = 1.4 s
//! high_cut = 6.5 kHz
//! freeze = off
//! ```
//!
//! `name` is the preset's name. Every other key names a parameter by its id
//! or its display name; case, spaces, dashes and underscores do not matter,
//! so `ReverbTime`, `reverb_time` and `Reverb Time` are one key. A value is
//! read by [`crate::ParamInfo::parse`], so it may be a choice name, `On` or
//! `Off`, or a number with a unit. A parameter the file leaves out keeps its
//! default.
//!
//! Parsing allocates; it is for start-up, never the audio thread.
//! [`crate::build::embed_files`] puts the files into the binary at build time.

use crate::PluginInfo;

/// One parsed preset: a name and a plain value for every parameter, in
/// descriptor order.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub values: Vec<f32>,
}

fn fold(key: &str) -> String {
    key.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The descriptor index a preset key names, by id first and then by name.
pub fn param_index(info: &PluginInfo, key: &str) -> Option<usize> {
    let key = fold(key);
    if key.is_empty() {
        return None;
    }
    info.params
        .iter()
        .position(|p| fold(p.id) == key)
        .or_else(|| info.params.iter().position(|p| fold(p.name) == key))
}

/// Parse one preset file. The error names the line at fault. A file with no
/// `name` line takes `fallback_name` (pass the file name).
pub fn parse(info: &PluginInfo, text: &str, fallback_name: &str) -> Result<Preset, String> {
    let mut name = None;
    let mut values: Vec<f32> = info.params.iter().map(|p| p.clamp(p.default)).collect();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = n + 1;
        let (key, value) = line.split_once('=').ok_or_else(|| format!("line {at}: expected key = value"))?;
        let (key, value) = (key.trim(), value.trim());
        if key.eq_ignore_ascii_case("name") {
            if value.is_empty() {
                return Err(format!("line {at}: empty name"));
            }
            name = Some(value.to_string());
            continue;
        }
        let index = param_index(info, key).ok_or_else(|| format!("line {at}: no parameter '{key}'"))?;
        let param = &info.params[index];
        values[index] = param.parse(value).ok_or_else(|| format!("line {at}: '{value}' is not a value for {}", param.name))?;
    }
    let name = name.unwrap_or_else(|| fallback_name.to_string());
    if name.is_empty() {
        return Err("the preset has no name".into());
    }
    Ok(Preset { name, values })
}

/// The text of a preset: its name, then every parameter that differs from
/// its default, by id, in the form [`crate::ParamInfo::format`] prints.
pub fn write(info: &PluginInfo, preset: &Preset) -> String {
    let mut out = format!("name = {}\n", preset.name);
    for (param, value) in info.params.iter().zip(&preset.values) {
        if param.clamp(*value) != param.clamp(param.default) {
            out.push_str(&format!("{} = {}\n", param.id, param.format(*value)));
        }
    }
    out
}

/// Parsed presets with `'static` names and values, as the [`crate::Plugin`]
/// preset methods hand them out. Build it once, in a `static OnceLock`; the
/// strings and slices live for the rest of the process.
#[derive(Debug, Default)]
pub struct PresetBank {
    names: Vec<&'static str>,
    values: Vec<&'static [f32]>,
}

impl PresetBank {
    /// Parse `(file name, text)` pairs (what [`crate::build::embed_files`]
    /// writes). A file that does not parse is left out and reported in the
    /// second result, so a test can insist that list is empty. Presets keep
    /// the order of `files`.
    pub fn parse_all(info: &PluginInfo, files: &[(&str, &str)]) -> (Self, Vec<String>) {
        let mut bank = Self::default();
        let mut errors = Vec::new();
        for (file, text) in files {
            let stem = file.rsplit(['/', '\\']).next().unwrap_or(file);
            let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
            match parse(info, text, stem) {
                Ok(preset) => {
                    bank.names.push(Box::leak(preset.name.into_boxed_str()));
                    bank.values.push(Box::leak(preset.values.into_boxed_slice()));
                }
                Err(e) => errors.push(format!("{file}: {e}")),
            }
        }
        (bank, errors)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// For [`crate::Plugin::preset_name`]: empty when out of range.
    pub fn name(&self, index: usize) -> &'static str {
        self.names.get(index).copied().unwrap_or("")
    }

    /// For [`crate::Plugin::preset_values`].
    pub fn values(&self, index: usize) -> Option<&'static [f32]> {
        self.values.get(index).copied()
    }

    /// The preset whose values these are, if any.
    pub fn find(&self, values: &[f32]) -> Option<usize> {
        self.values.iter().position(|v| *v == values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, ParamInfo, Unit};

    static MODES: [&str; 3] = ["Room", "Hall", "Plate"];
    static PARAMS: [ParamInfo; 4] = [
        ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::None).with_choices(&MODES),
        ParamInfo::new("reverb_time", "Reverb Time", 0.1, 20.0, 2.0, Unit::Seconds),
        ParamInfo::new("high_cut", "High Cut", 1_000.0, 20_000.0, 12_000.0, Unit::Hz),
        ParamInfo::new("freeze", "Freeze", 0.0, 1.0, 0.0, Unit::Toggle),
    ];
    static INFO: PluginInfo = PluginInfo {
        id: "test.verb",
        name: "Verb",
        vendor: "Test",
        url: "",
        email: "",
        version: "1.0.0",
        category: Category::Reverb,
        params: &PARAMS,
        is_instrument: false,
        has_editor: false,
    };

    #[test]
    fn keys_match_ids_and_names_loosely() {
        let text = "# hall\nname = Recital Hall\nmode = Hall\nReverbTime = 1400 ms\nHigh Cut = 6.5 kHz\nfreeze = off\n";
        let preset = parse(&INFO, text, "file").unwrap();
        assert_eq!(preset.name, "Recital Hall");
        assert_eq!(preset.values, vec![1.0, 1.4, 6_500.0, 0.0]);
    }

    #[test]
    fn defaults_fill_the_rest_and_the_file_name_names_it() {
        let preset = parse(&INFO, "mode = plate", "Bright Plate").unwrap();
        assert_eq!(preset.name, "Bright Plate");
        assert_eq!(preset.values, vec![2.0, 2.0, 12_000.0, 0.0]);
    }

    #[test]
    fn errors_name_the_line() {
        assert_eq!(parse(&INFO, "name = X\nsize = 3", "f").unwrap_err(), "line 2: no parameter 'size'");
        assert!(parse(&INFO, "mode = cathedral", "f").unwrap_err().starts_with("line 1:"));
        assert!(parse(&INFO, "just words", "f").unwrap_err().contains("key = value"));
    }

    #[test]
    fn write_then_parse_is_the_same_preset() {
        let preset = Preset { name: "Round Trip".into(), values: vec![2.0, 3.25, 8_000.0, 1.0] };
        let text = write(&INFO, &preset);
        assert!(text.contains("mode = Plate"));
        let back = parse(&INFO, &text, "x").unwrap();
        assert_eq!(back.name, preset.name);
        for (a, b) in back.values.iter().zip(&preset.values) {
            assert!((a - b).abs() <= b.abs() * 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn bank_serves_static_presets_and_reports_bad_files() {
        let files = [("Hall/Big.preset", "mode = Hall"), ("bad.preset", "nope = 1"), ("Room.preset", "name = Small\nmode = Room")];
        let (bank, errors) = PresetBank::parse_all(&INFO, &files);
        assert_eq!(bank.len(), 2);
        assert_eq!(bank.name(0), "Big");
        assert_eq!(bank.name(1), "Small");
        assert_eq!(bank.values(0).unwrap()[0], 1.0);
        assert_eq!(bank.name(5), "");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("bad.preset: line 1"));
        assert_eq!(bank.find(bank.values(1).unwrap()), Some(1));
    }
}
