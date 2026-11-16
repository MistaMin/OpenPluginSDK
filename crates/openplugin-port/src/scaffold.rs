//! `openplugin-port new`: from a scanner manifest to a plugin crate that
//! builds. The crate has the identity the SDK can pin today, the parameter
//! table the scanner inferred, a stub for reading the original's saved state
//! (`openplugin_core::foreign_state`), and a null test against the original
//! that runs with `cargo test`. Its audio passes the input through until the
//! DSP is written. Everything a person still has to do is a `TODO(porting`
//! line. See `docs/PORTING.md`.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::manifest::{Manifest, SCHEMA_VERSION};
use crate::snippet::{self, Layouts};

/// Where the generated crate gets the SDK from.
#[derive(Debug, Clone, PartialEq)]
pub enum SdkSource {
    /// `openplugin = { path = "<checkout>/crates/openplugin" }`.
    Path(PathBuf),
    /// `openplugin = "<version>"` from crates.io.
    Version(String),
}

#[derive(Debug, Clone)]
pub struct NewOptions {
    pub sdk: SdkSource,
    /// The Cargo package name; by default made from the plugin's name.
    pub crate_name: Option<String>,
    /// The `openplugin-port` executable the generated null test runs when
    /// `OPENPLUGIN_PORT` is not set. The CLI passes itself.
    pub tool: PathBuf,
}

/// One file of the generated crate, by its path inside the crate.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub path: &'static str,
    pub text: String,
}

/// Read a scanner manifest (`<name>.<format>.manifest.json`).
pub fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        let toml = path.extension().is_some_and(|x| x == "toml");
        format!("{} is not a scanner manifest: {e}{}", path.display(), if toml { " (the scanner writes JSON: pass the .manifest.json file `scan` wrote)" } else { "" })
    })?;
    let version = value.get("schema_version").and_then(|v| v.as_u64()).ok_or_else(|| format!("{} has no schema_version", path.display()))?;
    if version > u64::from(SCHEMA_VERSION) {
        return Err(format!("{} has manifest schema {version}; this tool reads up to {SCHEMA_VERSION}", path.display()));
    }
    serde_json::from_value(value).map_err(|e| format!("{} is not a scanner manifest: {e}", path.display()))
}

/// A Cargo package name from the plugin's name: lower case, `-` between words.
pub fn crate_name(m: &Manifest) -> String {
    let mut s = String::new();
    for c in m.plugin.name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() || s.as_bytes()[0].is_ascii_digit() { format!("port-{s}").trim_end_matches('-').to_string() } else { s }
}

fn valid_crate_name(name: &str) -> bool {
    !name.is_empty() && name.as_bytes()[0].is_ascii_alphabetic() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The plugin type's name: the crate name in upper camel case, then `Port`.
fn type_name(crate_name: &str) -> String {
    let mut s = String::new();
    for word in crate_name.split(['-', '_']).filter(|w| !w.is_empty()) {
        let mut cs = word.chars();
        if let Some(c) = cs.next() {
            s.push(c.to_ascii_uppercase());
            s.extend(cs);
        }
    }
    s + "Port"
}

/// A TOML basic string.
fn toml_str(text: &str) -> String {
    let mut s = String::from('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(s, "\\u{:04X}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// The plugin's version when it is `x.y.z` numbers, else `0.1.0`.
fn package_version(version: &str) -> String {
    let parts: Vec<&str> = version.trim().split('.').collect();
    if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() < 10 && p.bytes().all(|b| b.is_ascii_digit())) {
        parts.iter().map(|p| p.parse::<u64>().unwrap_or(0).to_string()).collect::<Vec<_>>().join(".")
    } else {
        "0.1.0".into()
    }
}

/// Every file of the crate for one manifest. Nothing is written.
pub fn generate(m: &Manifest, opts: &NewOptions) -> Result<Vec<File>, String> {
    let name = opts.crate_name.clone().unwrap_or_else(|| crate_name(m));
    if !valid_crate_name(&name) {
        return Err(format!("{name:?} is not a usable crate name: use letters, digits, - and _, starting with a letter"));
    }
    let ty = type_name(&name);
    let lib = name.replace('-', "_");
    let inferred = snippet::infer(&m.plugin);
    Ok(vec![
        File { path: "Cargo.toml", text: cargo_toml(m, &name, opts) },
        File { path: ".gitignore", text: "/target\n".into() },
        File { path: "README.md", text: readme(m, &name) },
        File { path: "src/params.rs", text: snippet::render(m) },
        File { path: "src/lib.rs", text: lib_rs(m, &ty) },
        File { path: "src/state_import.rs", text: state_import_rs(m, &inferred) },
        File { path: "tests/null.rs", text: null_rs(m, &lib, opts) },
    ])
}

/// Write the crate into `dir`, which must not exist or must be empty.
/// Returns the paths written.
pub fn write(files: &[File], dir: &Path) -> Result<Vec<PathBuf>, String> {
    if dir.exists() {
        let mut entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        if entries.next().is_some() {
            return Err(format!("{} exists and is not empty: nothing written", dir.display()));
        }
    }
    let mut written = Vec::new();
    for f in files {
        let path = dir.join(f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, &f.text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

fn cargo_toml(m: &Manifest, name: &str, opts: &NewOptions) -> String {
    let dep = match &opts.sdk {
        SdkSource::Path(p) => format!("{{ path = {} }}", toml_str(&p.join("crates").join("openplugin").display().to_string())),
        SdkSource::Version(v) => toml_str(v),
    };
    let mut s = String::new();
    let _ = writeln!(s, "# Generated by {} from the scan of {}.", m.tool, m.source.path);
    let _ = writeln!(s, "[package]");
    let _ = writeln!(s, "name = {}", toml_str(name));
    let _ = writeln!(s, "version = {}", toml_str(&package_version(&m.plugin.version)));
    let _ = writeln!(s, "description = {}", toml_str(&format!("OpenPluginSDK port of {}", m.plugin.name)));
    let _ = writeln!(s, "edition = \"2021\"");
    let _ = writeln!(s, "publish = false");
    let _ = writeln!(s);
    let _ = writeln!(s, "[lib]");
    let _ = writeln!(s, "# cdylib: CLAP and LV2; staticlib: linked into the VST3, Audio Unit and AAX shells (scripts/build_plugin.sh).");
    let _ = writeln!(s, "crate-type = [\"cdylib\", \"staticlib\", \"rlib\"]");
    let _ = writeln!(s);
    let _ = writeln!(s, "[dependencies]");
    let _ = writeln!(s, "openplugin = {dep}");
    let _ = writeln!(s);
    let _ = writeln!(s, "# A workspace of its own, so the crate builds wherever it is put. Remove this");
    let _ = writeln!(s, "# table to make the crate a member of an enclosing workspace.");
    let _ = writeln!(s, "[workspace]");
    s
}

fn readme(m: &Manifest, name: &str) -> String {
    let p = &m.plugin;
    let mut s = String::new();
    let _ = writeln!(s, "# {name}");
    let _ = writeln!(s);
    let _ = writeln!(s, "The OpenPluginSDK port of \"{}\"{}, started by `{}` from the {} scan of `{}`.", p.name, if p.vendor.is_empty() { String::new() } else { format!(" by {}", p.vendor) }, m.tool, m.source.format, m.source.path);
    let _ = writeln!(s);
    let _ = writeln!(s, "| File | What it holds |");
    let _ = writeln!(s, "|---|---|");
    let _ = writeln!(s, "| `src/params.rs` | The parameter table and `PluginInfo` inferred from the scan, with every guess explained; each parameter is pinned to the original's id where that is not its position (`openplugin::core::identity`). |");
    let _ = writeln!(s, "| `src/lib.rs` | The plugin: parameter storage, state, the original's VST3 class ids and Audio Unit and AAX codes where the scan read them, and a `process` that passes the input through until the DSP is written. |");
    let _ = writeln!(s, "| `src/state_import.rs` | The stub that reads the original's saved state (`openplugin_core::foreign_state`), with the map from the original's parameter ids to the port's. |");
    let _ = writeln!(s, "| `tests/null.rs` | The null test: renders the original and this port through the same input with `openplugin-port null` and fails when they differ. |");
    let _ = writeln!(s);
    let _ = writeln!(s, "`cargo test` runs the null test. It needs the `openplugin-port` tool (`OPENPLUGIN_PORT` names it; otherwise the one that wrote this crate, then `openplugin-port` on `PATH`) and the original plugin (`OPENPLUGIN_PORT_ORIGINAL` names it; otherwise the path it was scanned from).");
    let _ = writeln!(s);
    let _ = writeln!(s, "What is left: search for `TODO(porting`. In order: the DSP in `process` (the null test then proves it), the original's state format in `src/state_import.rs` (run `openplugin-port capture` on the original for test data), the identity lines in `src/params.rs`, and the automation in `tests/null.rs`. See `docs/PORTING.md` in the SDK.");
    s
}

fn lib_rs(m: &Manifest, ty: &str) -> String {
    let p = &m.plugin;
    let n = p.params.len();
    let mut s = String::new();
    let _ = writeln!(s, "//! The OpenPluginSDK port of {:?}{}.", p.name, if p.vendor.is_empty() { String::new() } else { format!(" by {:?}", p.vendor) });
    let _ = writeln!(s, "//!");
    let _ = writeln!(s, "//! Generated by {} from the {} scan of {}.", m.tool, m.source.format, m.source.path);
    let _ = writeln!(s, "//! Everything left to do is marked `TODO(porting`.");
    let _ = writeln!(s);
    let _ = writeln!(s, "mod params;");
    let _ = writeln!(s, "pub mod state_import;");
    let _ = writeln!(s);
    let _ = writeln!(s, "use openplugin::prelude::*;");
    let _ = writeln!(s);
    let _ = writeln!(s, "pub use params::{{INFO, PARAMS}};");
    let _ = writeln!(s);
    let _ = writeln!(s, "pub struct {ty} {{");
    let _ = writeln!(s, "    /// Each parameter's plain value, in table order.");
    let _ = writeln!(s, "    values: [f32; {n}],");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "impl Default for {ty} {{");
    let _ = writeln!(s, "    fn default() -> Self {{");
    let _ = writeln!(s, "        let mut values = [0.0; {n}];");
    let _ = writeln!(s, "        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {{");
    let _ = writeln!(s, "            *v = p.default;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        Self {{ values }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "impl {ty} {{");
    let _ = writeln!(s, "    fn index(id: &str) -> Option<usize> {{");
    let _ = writeln!(s, "        PARAMS.iter().position(|p| p.id == id)");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    /// Each parameter's plain value, in table order.");
    let _ = writeln!(s, "    pub fn values(&self) -> &[f32] {{");
    let _ = writeln!(s, "        &self.values");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "impl Plugin for {ty} {{");
    let _ = writeln!(s, "    fn info(&self) -> &'static PluginInfo {{");
    let _ = writeln!(s, "        &INFO");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {{}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    fn reset(&mut self) {{}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    fn set_param(&mut self, id: &str, value: f32) -> bool {{");
    let _ = writeln!(s, "        match Self::index(id) {{");
    let _ = writeln!(s, "            Some(i) => {{");
    let _ = writeln!(s, "                self.values[i] = PARAMS[i].clamp(value);");
    let _ = writeln!(s, "                true");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "            None => false,");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    fn param(&self, id: &str) -> Option<f32> {{");
    let _ = writeln!(s, "        Self::index(id).map(|i| self.values[i])");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {{");
    let _ = writeln!(s, "        // TODO(porting): the DSP. Until it is written the port passes its input through");
    let _ = writeln!(s, "        // unchanged, which nulls against the original only where the original does too.");
    let _ = writeln!(s, "        let _ = (io, frames);");
    let _ = writeln!(s, "    }}");
    match p.latency_samples {
        Some(0) | None => {}
        Some(l) => {
            let _ = writeln!(s);
            let _ = writeln!(s, "    // TODO(porting): the original reports {l} samples of latency. Return it from");
            let _ = writeln!(s, "    // `fn latency(&self) -> usize` once `process` delays its output by as much.");
        }
    }
    match p.tail_samples {
        Some(0) | None => {}
        Some(u32::MAX) => {
            let _ = writeln!(s);
            let _ = writeln!(s, "    // TODO(porting): the original reports an endless tail (`fn tail(&self) -> Tail` returning `Tail::Infinite`).");
        }
        Some(t) => {
            let _ = writeln!(s);
            let _ = writeln!(s, "    // TODO(porting): the original reports a tail of {t} samples (`fn tail(&self) -> Tail` returning `Tail::Finite({t})`).");
        }
    }
    match snippet::layouts(p) {
        Layouts::Config(expr) => {
            let _ = writeln!(s);
            let _ = writeln!(s, "    fn supported_layouts(&self) -> &'static [IoConfig] {{");
            let _ = writeln!(s, "        {expr}");
            let _ = writeln!(s, "    }}");
        }
        Layouts::ByHand(why) => {
            let _ = writeln!(s);
            let _ = writeln!(s, "    // TODO(porting): {why}");
        }
        Layouts::NoBuses => {}
    }
    let _ = writeln!(s);
    // ----- identity pins (openplugin::core::identity) -----
    // The CLAP id is PluginInfo::id and the parameter ids are pinned in the
    // table (src/params.rs); the class ids and codes are Plugin methods.
    match &p.ids.vst3 {
        Some(v) => {
            let _ = writeln!(s, "    // The original's VST3 class ids: a VST3 host finds the plugin in a project by the processor's.");
            if v.controller_class_id.is_none() {
                let _ = writeln!(s, "    // The original is a single-component VST3; the controller's id stays derived from PluginInfo::id.");
            }
            let _ = writeln!(s, "    fn vst3_ids(&self) -> Option<Vst3Ids> {{");
            let _ = writeln!(s, "        {}", snippet::vst3_ids_expr(v));
            let _ = writeln!(s, "    }}");
        }
        None => {
            let _ = writeln!(s, "    // TODO(porting): no VST3 class ids in the scan. If the original ships as VST3, scan its .vst3 and pin them with `fn vst3_ids`.");
        }
    }
    let _ = writeln!(s);
    match snippet::au_ids_expr(p) {
        Some(expr) => {
            let _ = writeln!(s, "    // The original's Audio Unit subtype and manufacturer codes.");
            let _ = writeln!(s, "    fn au_ids(&self) -> Option<AuIds> {{");
            let _ = writeln!(s, "        {expr}");
            let _ = writeln!(s, "    }}");
        }
        None if p.ids.au.is_some() => {
            let _ = writeln!(s, "    // TODO(porting): the original's Audio Unit codes are not four printable characters each (see src/params.rs); pass them to the build by hand.");
        }
        None => {
            let _ = writeln!(s, "    // TODO(porting): no Audio Unit codes in the scan. If the original ships as an Audio Unit, scan its .component and pin them with `fn au_ids`.");
        }
    }
    let _ = writeln!(s);
    match snippet::aax_ids_expr(p) {
        Some(expr) => {
            let _ = writeln!(s, "    // The original's AAX ids, from its bundle (the first two plug-in types as mono and stereo: check the page table).");
            let _ = writeln!(s, "    fn aax_ids(&self) -> Option<AaxIds> {{");
            let _ = writeln!(s, "        {expr}");
            let _ = writeln!(s, "    }}");
        }
        None if p.ids.aax.is_some() => {
            let _ = writeln!(s, "    // TODO(porting): the original's AAX ids are in src/params.rs, but not as four-character mono and stereo types; write `fn aax_ids` by hand.");
        }
        None => {
            let _ = writeln!(s, "    // TODO(porting): no AAX ids in the scan. If the original ships as AAX, pin its ids with `fn aax_ids` (Pro Tools finds sessions by them).");
        }
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "    fn save_state(&mut self) -> Option<Vec<u8>> {{");
    let _ = writeln!(s, "        Some(encode_params(&INFO, &self.values))");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    /// The original's state first (sessions and presets made before the port), then the");
    let _ = writeln!(s, "    /// port's own. A refused state changes nothing.");
    let _ = writeln!(s, "    fn load_state(&mut self, data: &[u8]) -> bool {{");
    let _ = writeln!(s, "        match state_import::import_original(data) {{");
    let _ = writeln!(s, "            Some(Ok(values)) => {{");
    let _ = writeln!(s, "                for (i, v) in values {{");
    let _ = writeln!(s, "                    self.values[i] = v;");
    let _ = writeln!(s, "                }}");
    let _ = writeln!(s, "                return true;");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "            Some(Err(_)) => return false,");
    let _ = writeln!(s, "            None => {{}}");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        match decode_params(&INFO, data) {{");
    let _ = writeln!(s, "            Some(values) if values.len() == self.values.len() => {{");
    let _ = writeln!(s, "                self.values.copy_from_slice(&values);");
    let _ = writeln!(s, "                true");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "            _ => false,");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "openplugin::targets::export_clap!({ty});");
    let _ = writeln!(s, "openplugin::targets::export_lv2!({ty});");
    let _ = writeln!(s, "openplugin::targets::export_shell!({ty});");
    let _ = writeln!(s);
    let _ = writeln!(s, "#[cfg(test)]");
    let _ = writeln!(s, "mod tests {{");
    let _ = writeln!(s, "    use super::*;");
    let _ = writeln!(s);
    let _ = writeln!(s, "    #[test]");
    let _ = writeln!(s, "    fn own_state_round_trips() {{");
    let _ = writeln!(s, "        let mut a = {ty}::default();");
    let _ = writeln!(s, "        for p in PARAMS.iter() {{");
    let _ = writeln!(s, "            assert!(a.set_param(p.id, p.max));");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        let blob = a.save_state().expect(\"a state\");");
    let _ = writeln!(s, "        let mut b = {ty}::default();");
    let _ = writeln!(s, "        assert!(b.load_state(&blob));");
    let _ = writeln!(s, "        assert_eq!(a.values(), b.values());");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    #[test]");
    let _ = writeln!(s, "    fn every_pin_is_usable() {{");
    let _ = writeln!(s, "        let plugin = {ty}::default();");
    let _ = writeln!(s, "        let errors = openplugin::core::identity::check_pins(&plugin);");
    let _ = writeln!(s, "        assert!(errors.is_empty(), \"{{errors:?}}\");");
    if p.ids.vst3.is_some() {
        let _ = writeln!(s, "        assert!(plugin.vst3_ids().is_some_and(|ids| ids.is_valid()), \"the VST3 class ids do not parse\");");
    }
    if snippet::au_ids_expr(p).is_some() {
        let _ = writeln!(s, "        assert!(plugin.au_ids().is_some_and(|ids| ids.is_valid()));");
    }
    if snippet::aax_ids_expr(p).is_some() {
        let _ = writeln!(s, "        assert!(plugin.aax_ids().is_some_and(|ids| ids.is_valid()));");
    }
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s);
    let _ = writeln!(s, "    #[test]");
    let _ = writeln!(s, "    fn a_refused_state_changes_nothing() {{");
    let _ = writeln!(s, "        let mut a = {ty}::default();");
    let _ = writeln!(s, "        let before = a.values().to_vec();");
    let _ = writeln!(s, "        assert!(!a.load_state(&[]));");
    let _ = writeln!(s, "        assert!(!a.load_state(b\"not a state\"));");
    let _ = writeln!(s, "        assert_eq!(a.values(), &before[..]);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    s
}

fn state_import_rs(m: &Manifest, inferred: &[snippet::Inferred]) -> String {
    let p = &m.plugin;
    let mut s = String::new();
    let _ = writeln!(s, "//! Reading the state the original {:?} saved, so the sessions and presets made", p.name);
    let _ = writeln!(s, "//! with it open in the port. The pattern is `openplugin_core::foreign_state`'s");
    let _ = writeln!(s, "//! (docs/PORTING.md, \"Foreign state import\"):");
    let _ = writeln!(s, "//!");
    let _ = writeln!(s, "//! 1. Run `openplugin-port capture` on the original and look at the blobs it saves.");
    let _ = writeln!(s, "//! 2. Make `original_detect` recognise them and `original_decode` read them, with");
    let _ = writeln!(s, "//!    the readers in `openplugin::core::foreign_state` (bytes, kv, chunk, xml, plist).");
    let _ = writeln!(s, "//! 3. Check `param_map` (made from the scan: the original's ids to the port's).");
    let _ = writeln!(s, "//! 4. Add a test that loads each captured blob and checks every parameter against");
    let _ = writeln!(s, "//!    the values the capture read back.");
    let _ = writeln!(s);
    let _ = writeln!(s, "use openplugin::core::foreign_state::import::{{import, ForeignId, ImportedState, ParamMap, StateFormat}};");
    let _ = writeln!(s, "use openplugin::core::foreign_state::{{ErrorKind, ForeignError, Result}};");
    let _ = writeln!(s);
    let _ = writeln!(s, "use crate::PARAMS;");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// The formats the port reads besides its own, tried in order.");
    let _ = writeln!(s, "pub static FORMATS: [StateFormat; 1] = [StateFormat {{ name: {:?}, detect: original_detect, decode: original_decode }}];", format!("{} (original)", p.name));
    let _ = writeln!(s);
    let _ = writeln!(s, "/// `Some(version)` when `data` is a state the original wrote. Look at the front only");
    let _ = writeln!(s, "/// (a magic, a version field, a root element).");
    let _ = writeln!(s, "fn original_detect(_data: &[u8]) -> Option<u32> {{");
    let _ = writeln!(s, "    // TODO(porting): recognise the original's state; until then nothing is taken for it.");
    let _ = writeln!(s, "    None");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// Read a state `original_detect` recognised into the original's parameter ids and values");
    let _ = writeln!(s, "/// as stored (`push_plain` or `push_normalized`). Return an error, never panic.");
    let _ = writeln!(s, "fn original_decode(_data: &[u8], _version: u32) -> Result<ImportedState> {{");
    let _ = writeln!(s, "    // TODO(porting): decode the original's state, starting with ImportedState::new(FORMATS[0].name, version).");
    let _ = writeln!(s, "    Err(ForeignError {{ kind: ErrorKind::Invalid, offset: 0, what: \"the original's state format is not read yet\" }})");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// The original's parameter ids (the scan's `params[].id`) to the port's `ParamInfo` ids.");
    let _ = writeln!(s, "/// The port's range is the original's plain range unless src/params.rs says it was read");
    let _ = writeln!(s, "/// from the value strings: then use `map_with` and a conversion.");
    let _ = writeln!(s, "pub fn param_map() -> ParamMap {{");
    let _ = writeln!(s, "    let mut m = ParamMap::new();");
    for (src, inf) in p.params.iter().zip(inferred) {
        let _ = writeln!(s, "    m.map(ForeignId::Index({}), {:?}); // {:?}", src.id, inf.id, src.name);
    }
    let _ = writeln!(s, "    m");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// `None` when no format recognises `data` (the port's own format is tried next);");
    let _ = writeln!(s, "/// `Some(Err(_))` when it was recognised but is damaged or names parameters the map does");
    let _ = writeln!(s, "/// not place (refuse it); otherwise the values to set, as table indices and plain values.");
    let _ = writeln!(s, "pub fn import_original(data: &[u8]) -> Option<std::result::Result<Vec<(usize, f32)>, String>> {{");
    let _ = writeln!(s, "    let state = match import(&FORMATS, data)? {{");
    let _ = writeln!(s, "        Ok(state) => state,");
    let _ = writeln!(s, "        Err(e) => return Some(Err(e.to_string())),");
    let _ = writeln!(s, "    }};");
    let _ = writeln!(s, "    let applied = param_map().apply(&PARAMS, &state);");
    let _ = writeln!(s, "    if !applied.unmapped.is_empty() {{");
    let _ = writeln!(s, "        return Some(Err(format!(\"parameters with no place in the port: {{:?}}\", applied.unmapped)));");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    Some(Ok(applied.values))");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "#[cfg(test)]");
    let _ = writeln!(s, "mod tests {{");
    let _ = writeln!(s, "    use super::*;");
    let _ = writeln!(s);
    let _ = writeln!(s, "    #[test]");
    let _ = writeln!(s, "    fn every_parameter_of_the_original_has_a_place() {{");
    let _ = writeln!(s, "        let mut state = ImportedState::new(\"test\", 1);");
    let _ = writeln!(s, "        let ids: [u32; {}] = [{}];", p.params.len(), p.params.iter().map(|x| x.id.to_string()).collect::<Vec<_>>().join(", "));
    let _ = writeln!(s, "        for (id, info) in ids.iter().zip(PARAMS.iter()) {{");
    let _ = writeln!(s, "            state.push_plain(ForeignId::Index(*id), f64::from(info.default));");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        let applied = param_map().apply(&PARAMS, &state);");
    let _ = writeln!(s, "        assert!(applied.unmapped.is_empty(), \"{{:?}}\", applied.unmapped);");
    let _ = writeln!(s, "        assert_eq!(applied.values.len(), PARAMS.len());");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    s
}

fn null_rs(m: &Manifest, lib: &str, opts: &NewOptions) -> String {
    let p = &m.plugin;
    let mut s = String::new();
    let _ = writeln!(s, "//! The null test: the original {:?} and this port render the same input, and", p.name);
    let _ = writeln!(s, "//! `openplugin-port null` compares them (docs/PORTING.md, \"The null test\").");
    let _ = writeln!(s, "//!");
    let _ = writeln!(s, "//! `OPENPLUGIN_PORT` names the `openplugin-port` executable (otherwise the one that");
    let _ = writeln!(s, "//! generated this crate, then `openplugin-port` on `PATH`); `OPENPLUGIN_PORT_ORIGINAL`");
    let _ = writeln!(s, "//! names the original plugin (otherwise the path it was scanned from).");
    let _ = writeln!(s);
    let _ = writeln!(s, "use std::path::{{Path, PathBuf}};");
    let _ = writeln!(s, "use std::process::Command;");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// The plugin the scan read.");
    let _ = writeln!(s, "const ORIGINAL: &str = {:?};", m.source.path);
    let _ = writeln!(s, "const ORIGINAL_FORMAT: &str = {:?};", m.source.format);
    if m.source.count > 1 {
        let _ = writeln!(s, "// TODO(porting): the original's bundle holds {} plugins and this port is number {}; the null test renders the bundle's first.", m.source.count, m.source.index + 1);
    }
    let _ = writeln!(s, "/// The `openplugin-port` that generated this crate.");
    let _ = writeln!(s, "const TOOL: &str = {:?};", opts.tool.display().to_string());
    let _ = writeln!(s, "/// This crate's library, as Cargo names it.");
    let _ = writeln!(s, "const LIB: &str = {lib:?};");
    let _ = writeln!(s, "/// Built-in signals, each rendered and compared (`sweep`, `noise`, `impulses`, `burst`, `silence`, `:seconds`).");
    let _ = writeln!(s, "const INPUTS: &[&str] = &[\"sweep:1\", \"noise:1\", \"impulses:1\"];");
    let _ = writeln!(s, "/// Largest sample difference allowed.");
    let _ = writeln!(s, "const TOLERANCE: &str = \"1e-6\";");
    let example: Vec<String> = p.params.iter().filter(|x| !x.has_flag("read_only")).take(2).enumerate().map(|(i, x)| format!("{}:{}={}", i * 4800, x.name, x.default)).collect();
    let _ = writeln!(s, "/// Parameter moves for both plugins, as `frame:param=value` (the original's names or ids,");
    let _ = writeln!(s, "/// plain values), separated by commas; empty compares at the defaults.");
    if !example.is_empty() {
        let _ = writeln!(s, "// TODO(porting): once the DSP is written, move every parameter here, such as {:?}.", example.join(", "));
    }
    let _ = writeln!(s, "const AUTOMATION: &str = \"\";");
    let _ = writeln!(s);
    let _ = writeln!(s, "fn tool() -> PathBuf {{");
    let _ = writeln!(s, "    if let Some(t) = std::env::var_os(\"OPENPLUGIN_PORT\") {{");
    let _ = writeln!(s, "        return PathBuf::from(t);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    if Path::new(TOOL).is_file() {{");
    let _ = writeln!(s, "        return PathBuf::from(TOOL);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    PathBuf::from(\"openplugin-port\")");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "fn original() -> PathBuf {{");
    let _ = writeln!(s, "    let path = std::env::var_os(\"OPENPLUGIN_PORT_ORIGINAL\").map_or_else(|| PathBuf::from(ORIGINAL), PathBuf::from);");
    let _ = writeln!(s, "    assert!(path.exists(), \"the original plugin {{}} is not there: set OPENPLUGIN_PORT_ORIGINAL\", path.display());");
    let _ = writeln!(s, "    path");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// Build this crate's library and return the file Cargo reports for it (Cargo decides the");
    let _ = writeln!(s, "/// target folder and its layout, from CARGO_TARGET_DIR or its configuration).");
    let _ = writeln!(s, "fn port() -> PathBuf {{");
    let _ = writeln!(s, "    let cargo = std::env::var(\"CARGO\").unwrap_or_else(|_| \"cargo\".into());");
    let _ = writeln!(s, "    let out = Command::new(cargo)");
    let _ = writeln!(s, "        .args([\"build\", \"--lib\", \"--message-format=json\", \"--manifest-path\"])");
    let _ = writeln!(s, "        .arg(Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"Cargo.toml\"))");
    let _ = writeln!(s, "        .output()");
    let _ = writeln!(s, "        .expect(\"run cargo\");");
    let _ = writeln!(s, "    assert!(out.status.success(), \"building the port failed:\\n{{}}\", String::from_utf8_lossy(&out.stderr));");
    let _ = writeln!(s, "    let name = format!(\"{{}}{{LIB}}{{}}\", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);");
    let _ = writeln!(s, "    let messages = String::from_utf8_lossy(&out.stdout);");
    let _ = writeln!(s, "    let lib = messages");
    let _ = writeln!(s, "        .lines()");
    let _ = writeln!(s, "        .filter(|l| l.contains(\"\\\"reason\\\":\\\"compiler-artifact\\\"\"))");
    let _ = writeln!(s, "        .flat_map(json_strings)");
    let _ = writeln!(s, "        .map(PathBuf::from)");
    let _ = writeln!(s, "        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name))");
    let _ = writeln!(s, "        .unwrap_or_else(|| panic!(\"cargo did not report {{name}}\"));");
    let _ = writeln!(s, "    assert!(lib.is_file(), \"{{}} was not built\", lib.display());");
    let _ = writeln!(s, "    lib");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    let _ = writeln!(s, "/// Every string value in one line of Cargo's JSON, unescaped (enough for paths).");
    let _ = writeln!(s, "fn json_strings(line: &str) -> Vec<String> {{");
    let _ = writeln!(s, "    let mut out = Vec::new();");
    let _ = writeln!(s, "    let mut chars = line.chars();");
    let _ = writeln!(s, "    while let Some(c) = chars.next() {{");
    let _ = writeln!(s, "        if c != '\"' {{");
    let _ = writeln!(s, "            continue;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        let mut text = String::new();");
    let _ = writeln!(s, "        while let Some(c) = chars.next() {{");
    let _ = writeln!(s, "            match c {{");
    let _ = writeln!(s, "                '\"' => break,");
    let _ = writeln!(s, "                '\\\\' => text.push(chars.next().unwrap_or('\\\\')),");
    let _ = writeln!(s, "                c => text.push(c),");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        out.push(text);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    out");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
    if m.source.format == "aax" {
        let _ = writeln!(s, "#[test]");
        let _ = writeln!(s, "#[ignore = \"an AAX plug-in cannot be rendered offline; scan and null the original's CLAP, VST3 or Audio Unit instead\"]");
    } else {
        let _ = writeln!(s, "#[test]");
    }
    let _ = writeln!(s, "fn the_port_nulls_against_the_original() {{");
    let _ = writeln!(s, "    let (tool, original, port) = (tool(), original(), port());");
    let _ = writeln!(s, "    for input in INPUTS {{");
    let _ = writeln!(s, "        let mut null = Command::new(&tool);");
    let _ = writeln!(s, "        null.arg(\"null\").arg(&original).arg(&port);");
    let _ = writeln!(s, "        null.args([\"--format\", ORIGINAL_FORMAT, \"--port-format\", \"clap\", \"--input\", input, \"--tolerance\", TOLERANCE]);");
    let _ = writeln!(s, "        if !AUTOMATION.is_empty() {{");
    let _ = writeln!(s, "            null.args([\"--automation\", AUTOMATION]);");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        let out = null.output().unwrap_or_else(|e| panic!(\"cannot run {{}}: {{e}} (set OPENPLUGIN_PORT)\", tool.display()));");
    let _ = writeln!(s, "        let report = String::from_utf8_lossy(&out.stdout);");
    let _ = writeln!(s, "        println!(\"input {{input}}:\\n{{report}}\");");
    let _ = writeln!(s, "        assert!(out.status.success(), \"input {{input}}: the port does not null against the original ({{}})\\n{{report}}{{}}\", out.status, String::from_utf8_lossy(&out.stderr));");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Manifest {
        let mut m = Manifest::default();
        m.plugin.name = name.into();
        m
    }

    #[test]
    fn names() {
        assert_eq!(crate_name(&named("Example Gain")), "example-gain");
        assert_eq!(crate_name(&named("  Über -- Verb 2 ")), "ber-verb-2");
        assert_eq!(crate_name(&named("1176 Comp")), "port-1176-comp");
        assert_eq!(crate_name(&named("")), "port");
        assert_eq!(type_name("example-gain"), "ExampleGainPort");
        assert_eq!(type_name("port_1176-comp"), "Port1176CompPort");
        assert!(valid_crate_name("a-b_c") && !valid_crate_name("1a") && !valid_crate_name("a b") && !valid_crate_name(""));
    }

    #[test]
    fn versions_and_toml_strings() {
        assert_eq!(package_version("1.2.3"), "1.2.3");
        assert_eq!(package_version("01.2.30"), "1.2.30");
        assert_eq!(package_version("1.2"), "0.1.0");
        assert_eq!(package_version("1.2.3b"), "0.1.0");
        assert_eq!(toml_str(r#"C:\Program Files\"x""#), r#""C:\\Program Files\\\"x\"""#);
        assert_eq!(toml_str("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn a_bad_crate_name_is_refused() {
        let opts = NewOptions { sdk: SdkSource::Version("0.4.0".into()), crate_name: Some("9 lives".into()), tool: PathBuf::from("openplugin-port") };
        assert!(generate(&named("X"), &opts).is_err());
    }

    #[test]
    fn an_aax_original_gets_an_ignored_null_test_and_its_ids_pinned() {
        let mut m = named("Verb");
        m.source.format = "aax".into();
        m.plugin.ids.aax = Some(crate::manifest::AaxIds { manufacturer: "Acme".into(), product: "Verb".into(), plugin_ids: vec!["VbM1".into(), "VbS1".into()], found_in: "page-table".into() });
        let opts = NewOptions { sdk: SdkSource::Version("0.4.0".into()), crate_name: None, tool: PathBuf::from("openplugin-port") };
        let files = generate(&m, &opts).unwrap();
        let get = |p: &str| files.iter().find(|f| f.path == p).unwrap().text.clone();
        assert!(get("tests/null.rs").contains("#[ignore = \"an AAX plug-in cannot be rendered offline"));
        assert!(get("src/lib.rs").contains(r#"Some(AaxIds { manufacturer: *b"Acme", product: *b"Verb", mono: *b"VbM1", stereo: *b"VbS1" })"#));
        assert!(get("Cargo.toml").contains("openplugin = \"0.4.0\""));
    }
}
