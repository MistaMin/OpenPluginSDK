//! Foreign state import, end to end. `openplugin-port capture` saves the SDK's
//! own example (the stand-in for "the old plugin") in CLAP, VST3 and Audio Unit
//! form; a test "port" plugin, with its parameters in another order, loads the
//! captured blobs through the documented `load_state` pattern
//! (`openplugin_core::foreign_state`), and every parameter must equal the
//! plain value the capture read back from the old plugin.
//!
//! CLAP always runs (the test builds the example's `cdylib`). VST3 and Audio
//! Unit run when `OPENPLUGIN_PORT_FIXTURES` names the folder of built bundles
//! (see `docs/PORTING.md`) and print "skipped" otherwise.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use openplugin_core::foreign_state::bytes::{Endian, Reader};
use openplugin_core::foreign_state::chunk::has_magic;
use openplugin_core::foreign_state::import::{ForeignId, ImportedState, ParamMap, StateFormat, import};
use openplugin_core::foreign_state::plist::{self, Value};
use openplugin_core::foreign_state::{ErrorKind, ForeignError};
use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_port::capture::{self, CAPTURE_SCHEMA_VERSION, Capture, CaptureOptions, SetSpec, fnv1a64};
use openplugin_port::{ScanOptions, scan};

// ---------- the port ----------

/// The port's table: the same two parameters as the old plugin, in the other order.
static PORT_PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle),
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
];

static PORT_INFO: PluginInfo = PluginInfo {
    id: "com.example.ported-gain",
    name: "Ported Gain",
    vendor: "Example",
    url: "",
    email: "",
    version: "1.0.0",
    category: Category::Utility,
    params: &PORT_PARAMS,
    is_instrument: false,
    has_editor: false,
};

const OWN_MAGIC: &[u8; 4] = b"PGv1";

/// How the old plugin names its parameters: by the scanner manifest's ids.
fn param_map() -> ParamMap {
    let mut m = ParamMap::new();
    m.map(ForeignId::Index(0), "gain").map(ForeignId::Index(1), "warm");
    m
}

// Format 1, "OPST" v1: magic, u16 version, u16 count, count f32 plain values; all little-endian.
fn plane_detect(data: &[u8]) -> Option<u32> {
    (has_magic(data, b"OPST") && data.len() >= 8).then(|| u32::from(u16::from_le_bytes([data[4], data[5]])))
}

fn plane_decode(data: &[u8], version: u32) -> Result<ImportedState, ForeignError> {
    let mut r = Reader::little(data);
    r.skip(6)?;
    let count = r.u16()?;
    r.check_count(u64::from(count), 4)?;
    let mut s = ImportedState::new("plane", version);
    for i in 0..count {
        s.push_plain(ForeignId::Index(u32::from(i)), f64::from(r.f32()?));
    }
    if !r.is_empty() {
        return Err(ForeignError { kind: ErrorKind::Invalid, offset: r.position(), what: "bytes after the last value" });
    }
    Ok(s)
}

// Format 2, a VST3 component state: i32 version (1), i32 count, count f64 normalized values, i32 bypass,
// then optionally a trailer that carries the old plugin's own blob (ignored here).
fn head_detect(data: &[u8]) -> Option<u32> {
    let mut r = Reader::little(data);
    let (v, n) = (r.i32().ok()?, r.i32().ok()?);
    (v == 1 && (0..=1024).contains(&n)).then_some(1)
}

fn head_decode(data: &[u8], version: u32) -> Result<ImportedState, ForeignError> {
    let mut r = Reader::little(data);
    r.skip(4)?;
    let count = r.i32()?;
    r.check_count(count as u64, 8)?;
    let mut s = ImportedState::new("vst3-head", version);
    for i in 0..count {
        s.push_normalized(ForeignId::Index(i as u32), r.f64()?);
    }
    let _bypass = r.i32()?;
    if !r.is_empty() {
        // A trailer: tag, version, length, blob. Check it is whole; the port does not use it.
        r.skip(4)?;
        let _v = r.i32()?;
        let len = r.u32()?;
        r.check_count(u64::from(len), 1)?;
        s.chunk = r.bytes(len as usize)?.to_vec();
        if !r.is_empty() {
            return Err(ForeignError { kind: ErrorKind::Invalid, offset: r.position(), what: "bytes after the trailer" });
        }
    }
    Ok(s)
}

// Format 3, an Audio Unit ClassInfo property list: `data` holds three big-endian u32 (version, reserved,
// number of pairs) and then (u32 parameter id, f32 plain value) pairs, big-endian.
fn au_detect(data: &[u8]) -> Option<u32> {
    if !plist::is_binary(data) {
        return None;
    }
    let v = plist::parse(data).ok()?;
    v.get("data").and_then(Value::as_data)?;
    v.get("version").and_then(Value::as_integer).map(|v| v as u32)
}

fn au_decode(data: &[u8], version: u32) -> Result<ImportedState, ForeignError> {
    let bad = |what| ForeignError { kind: ErrorKind::Invalid, offset: 0, what };
    let doc = plist::parse(data)?;
    let blob = doc.get("data").and_then(Value::as_data).ok_or_else(|| bad("no data key"))?;
    let mut r = Reader::big(blob);
    r.skip(8)?;
    let n = r.u32()?;
    r.check_count(u64::from(n), 8)?;
    let mut s = ImportedState::new("au-classinfo", version);
    for _ in 0..n {
        let id = r.u32()?;
        s.push_plain(ForeignId::Index(id), f64::from(r.f32()?));
    }
    Ok(s)
}

static FORMATS: [StateFormat; 3] = [
    StateFormat { name: "plane", detect: plane_detect, decode: plane_decode },
    StateFormat { name: "au-classinfo", detect: au_detect, decode: au_decode },
    StateFormat { name: "vst3-head", detect: head_detect, decode: head_decode },
];

struct PortedGain {
    values: [f32; 2],
}

impl PortedGain {
    fn new() -> Self {
        Self { values: [PORT_PARAMS[0].default, PORT_PARAMS[1].default] }
    }

    fn save_own(&self) -> Vec<u8> {
        let mut out = OWN_MAGIC.to_vec();
        for v in self.values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    fn load_own(&mut self, data: &[u8]) -> bool {
        if !has_magic(data, OWN_MAGIC) || data.len() != 4 + 8 {
            return false;
        }
        let mut next = self.values;
        for (i, v) in next.iter_mut().enumerate() {
            let x = f32::from_le_bytes(data[4 + i * 4..8 + i * 4].try_into().unwrap());
            if !x.is_finite() {
                return false;
            }
            *v = PORT_PARAMS[i].clamp(x);
        }
        self.values = next;
        true
    }
}

impl Plugin for PortedGain {
    fn info(&self) -> &'static PluginInfo {
        &PORT_INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match PORT_PARAMS.iter().position(|p| p.id == id) {
            Some(i) => {
                self.values[i] = PORT_PARAMS[i].clamp(value);
                true
            }
            None => false,
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        PORT_PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}

    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(self.save_own())
    }

    /// The pattern: a recognised old format is decoded, mapped and checked completely before anything
    /// changes; otherwise the port's own format.
    fn load_state(&mut self, data: &[u8]) -> bool {
        match import(&FORMATS, data) {
            Some(Ok(state)) => {
                let applied = param_map().apply(&PORT_PARAMS, &state);
                if !applied.unmapped.is_empty() || applied.values.len() != PORT_PARAMS.len() {
                    return false;
                }
                for (index, value) in applied.values {
                    self.values[index] = value;
                }
                true
            }
            Some(Err(_)) => false,
            None => self.load_own(data),
        }
    }
}

// ---------- capture helpers ----------

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_openplugin-port"))
}

fn example_dylib() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo).args(["build", "-p", "example-editor", "--lib"]).current_dir(&root).status().expect("run cargo");
        assert!(status.success(), "building the example failed");
        let target = std::env::var_os("CARGO_TARGET_DIR").map(|t| root.join(t)).unwrap_or_else(|| root.join("target"));
        let dylib = target.join("debug").join(format!("{}example_editor{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX));
        assert!(dylib.is_file(), "{} was not built", dylib.display());
        dylib
    })
}

fn fixtures() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENPLUGIN_PORT_FIXTURES")?);
    dir.is_dir().then_some(dir)
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::current_exe().unwrap().parent().unwrap().join("port-state-test").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn capture_options(format: &str, out: PathBuf) -> CaptureOptions {
    let mut o = CaptureOptions::new(helper(), out);
    o.format = Some(format.into());
    o.sets = vec![SetSpec { param: "gain".into(), value: -6.0 }, SetSpec { param: "Warm".into(), value: 1.0 }];
    o
}

/// Capture, check the manifest and blobs against themselves, and return the manifest and its folder.
fn capture_checked(path: &Path, format: &str, name: &str) -> (Capture, PathBuf) {
    let out = out_dir(name);
    let mut results = capture::run(path, &capture_options(format, out.clone())).expect("capture");
    assert_eq!(results.len(), 1);
    let (cap, file) = results.remove(0);
    // The file on disk is the same manifest.
    let on_disk: Capture = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(on_disk, cap);
    assert_eq!(cap.schema_version, CAPTURE_SCHEMA_VERSION);
    assert_eq!(cap.source.format, format);
    assert!(["Editor Gain", "Example Gain"].contains(&cap.plugin.name.as_str()), "{}", cap.plugin.name);
    // Default plus the --set state.
    let names: Vec<_> = cap.states.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["default", "set"], "{:?}", cap.notes);
    for state in &cap.states {
        assert!(!state.blobs.is_empty());
        // Every parameter is read back; VST3 adds the host bypass parameter.
        let expected = if format == "vst3" { 3 } else { 2 };
        assert_eq!(state.params.len(), expected, "{:?}", state.params);
        for b in &state.blobs {
            let bytes = std::fs::read(out.join(&b.file)).unwrap();
            assert_eq!(bytes.len() as u64, b.bytes);
            assert_eq!(fnv1a64(&bytes), b.fnv1a64);
            assert!(!bytes.is_empty());
        }
    }
    // The values read back are what the state holds.
    let value = |state: usize, id: u32| cap.states[state].params.iter().find(|p| p.id == id).unwrap().plain;
    assert_eq!((value(0, 0), value(0, 1)), (0.0, 0.0));
    assert!((value(1, 0) + 6.0).abs() < 1e-4 && (value(1, 1) - 1.0).abs() < 1e-9, "{:?}", cap.states[1].params);
    (cap, out)
}

fn blob(cap: &Capture, dir: &Path, state: usize, role: &str) -> Vec<u8> {
    let b = cap.states[state].blobs.iter().find(|b| b.role == role).unwrap_or_else(|| panic!("no {role} blob"));
    std::fs::read(dir.join(&b.file)).unwrap()
}

/// The scanner's ids map onto the port's table by name, and the hard-coded map agrees.
fn assert_map_matches_scan(path: &Path, format: &str) {
    let mut o = ScanOptions::new(helper());
    o.format = Some(format.into());
    let m = scan(path, &o).expect("scan").remove(0);
    let map = param_map();
    for p in m.plugin.params.iter().filter(|p| !p.has_flag("bypass")) {
        let state = {
            let mut s = ImportedState::new("probe", 0);
            s.push_plain(ForeignId::Index(p.id), p.default);
            s
        };
        let applied = map.apply(&PORT_PARAMS, &state);
        assert_eq!(applied.values.len(), 1, "scanner id {} ({}) is not mapped", p.id, p.name);
        assert_eq!(PORT_PARAMS[applied.values[0].0].name, p.name);
    }
}

/// Load every captured state into a fresh port; every parameter must equal the captured plain value.
fn assert_port_matches(cap: &Capture, dir: &Path, role: &str, tolerance: f64) {
    for (i, state) in cap.states.iter().enumerate() {
        let bytes = blob(cap, dir, i, role);
        let mut port = PortedGain::new();
        // Start from values the state does not have, so a no-op load cannot pass.
        port.set_param("gain", 17.0);
        port.set_param("warm", 1.0);
        assert!(port.load_state(&bytes), "state {:?} was refused", state.name);
        for p in state.params.iter().filter(|p| p.id < 2) {
            let target = param_map_target(p.id);
            let got = f64::from(port.param(target).unwrap());
            assert!((got - p.plain).abs() <= tolerance, "state {:?}, parameter {} ({}): port {got}, captured {}", state.name, p.id, p.name, p.plain);
        }
        // The port saves its own format from now on, and reads it back.
        let own = port.save_state().unwrap();
        let mut again = PortedGain::new();
        assert!(again.load_state(&own));
        assert_eq!(again.values, port.values);
    }
}

fn param_map_target(id: u32) -> &'static str {
    match id {
        0 => "gain",
        1 => "warm",
        other => panic!("unexpected id {other}"),
    }
}

/// Truncated and corrupted copies of a captured blob never panic, and a refusal changes nothing.
fn assert_damage_is_safe(bytes: &[u8]) {
    let mut start = PortedGain::new();
    start.set_param("gain", 3.0);
    start.set_param("warm", 1.0);
    let before = start.values;
    let check = |data: &[u8]| {
        let mut p = PortedGain::new();
        p.values = before;
        let ok = p.load_state(data);
        if !ok {
            assert_eq!(p.values, before, "a refused state changed a parameter");
        }
        ok
    };
    for n in 0..bytes.len() {
        check(&bytes[..n]);
    }
    let mut refused = 0;
    for i in 0..bytes.len() {
        let mut d = bytes.to_vec();
        d[i] ^= 0xFF;
        if !check(&d) {
            refused += 1;
        }
    }
    assert!(refused > 0, "no single-byte corruption was refused");
    let mut extended = bytes.to_vec();
    extended.extend_from_slice(&[0; 3]);
    check(&extended);
    assert!(!check(&[]));
}

// ---------- the tests ----------

#[test]
fn clap_capture_loads_into_the_port() {
    let (cap, dir) = capture_checked(example_dylib(), "clap", "clap");
    assert_eq!(cap.states[0].blobs[0].role, "clap-state");
    let bytes = blob(&cap, &dir, 1, "clap-state");
    // What the plugin wrote: OPST, version 1, two values, -6 and 1.
    assert_eq!(&bytes[..6], b"OPST\x01\x00");
    assert_eq!(bytes.len(), 6 + 2 + 8);
    assert_map_matches_scan(example_dylib(), "clap");
    assert_port_matches(&cap, &dir, "clap-state", 1e-6);
    assert_damage_is_safe(&bytes);
    // The old format is recognised by its magic, and a state from the port's own format still loads.
    assert_eq!(plane_detect(&bytes), Some(1));
    assert!(import(&FORMATS, b"PGv1....").is_none());
}

#[test]
#[cfg_attr(windows, ignore = "needs the Windows VST3 shell (native/vst3/win)")]
fn vst3_capture_loads_into_the_port() {
    let Some(f) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    let path = f.join("Example Gain.vst3");
    let (cap, dir) = capture_checked(&path, "vst3", "vst3");
    let roles: Vec<_> = cap.states[1].blobs.iter().map(|b| b.role.as_str()).collect();
    assert!(roles.contains(&"vst3-component"), "{roles:?}");
    // VST3 reports normalised values: the capture holds both.
    let gain = cap.states[1].params.iter().find(|p| p.id == 0).unwrap();
    assert!((gain.normalized.unwrap() - 0.375).abs() < 1e-9);
    let bytes = blob(&cap, &dir, 1, "vst3-component");
    assert_map_matches_scan(&path, "vst3");
    assert_port_matches(&cap, &dir, "vst3-component", 1e-5);
    assert_damage_is_safe(&bytes);
    assert_eq!(head_detect(&bytes), Some(1));
    // The component state ends in a trailer holding the plugin's own blob.
    assert!(!head_decode(&bytes, 1).unwrap().chunk.is_empty());
}

#[test]
#[cfg_attr(windows, ignore = "Audio Units exist only on macOS")]
fn audio_unit_capture_loads_into_the_port() {
    let Some(f) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    let path = f.join("Example Gain.component");
    let (cap, dir) = capture_checked(&path, "au", "au");
    let bytes = blob(&cap, &dir, 1, "au-classinfo");
    // A binary property list, as hosts store it, readable with the plist reader.
    assert!(plist::is_binary(&bytes));
    let doc = plist::parse(&bytes).expect("ClassInfo parses");
    assert_eq!(doc.get("type").and_then(Value::as_integer), Some(i128::from(u32::from_be_bytes(*b"aufx"))));
    assert_eq!(doc.get("subtype").and_then(Value::as_integer), Some(i128::from(u32::from_be_bytes(*b"ExGn"))));
    assert_map_matches_scan(&path, "au");
    assert_port_matches(&cap, &dir, "au-classinfo", 1e-6);
    assert_damage_is_safe(&bytes);
}

#[test]
fn a_plugin_that_crashes_fails_the_capture_and_nothing_else() {
    let mut o = capture_options("clap", out_dir("crash"));
    o.env = vec![(openplugin_port::probe::FAULT_ENV.into(), "crash".into())];
    let err = capture::run(example_dylib(), &o).unwrap_err();
    assert!(err.contains("crashed"), "{err}");
    assert!(capture::run(example_dylib(), &capture_options("clap", out_dir("after-crash"))).is_ok());
}

#[test]
fn capture_refuses_what_it_cannot_do() {
    let o = capture_options("aax", out_dir("aax"));
    assert!(capture::run(example_dylib(), &o).unwrap_err().contains("cannot capture"));
    let mut o = capture_options("clap", out_dir("bad-set"));
    o.sets = vec![SetSpec { param: "no such parameter".into(), value: 1.0 }];
    let err = capture::run(example_dylib(), &o).unwrap_err();
    assert!(err.contains("no parameter"), "{err}");
    assert!(capture::parse_set("gain=-3").is_ok());
    assert!(capture::parse_set("gain").is_err());
    assert!(capture::parse_set("gain=abc").is_err());
    assert!(capture::parse_set("gain=nan").is_err());
}

#[test]
fn foreign_ids_that_the_map_does_not_know_are_reported() {
    let mut s = ImportedState::new("t", 1);
    s.push_plain(ForeignId::Index(0), 3.0);
    s.push_plain(ForeignId::Index(9), 1.0);
    s.push_plain(ForeignId::Name("nope".into()), 1.0);
    s.push_plain(ForeignId::Index(1), f64::NAN);
    let applied = param_map().apply(&PORT_PARAMS, &s);
    assert_eq!(applied.values, vec![(1, 3.0)]);
    assert_eq!(applied.unmapped.len(), 3);
    // A decoder that sees a big-endian file reads it with the other byte order.
    let mut r = Reader::new(&[0, 0, 1, 0], Endian::Big);
    assert_eq!(r.u32().unwrap(), 256);
}

/// Read-only smoke over captures of installed plugins: every Audio Unit ClassInfo blob in the folder named
/// by `OPENPLUGIN_PORT_SMOKE_DIR` (made with `openplugin-port capture`) must read with the plist reader.
#[test]
fn captured_class_info_blobs_of_other_plugins_read_with_the_plist_reader() {
    let Some(dir) = std::env::var_os("OPENPLUGIN_PORT_SMOKE_DIR").map(PathBuf::from) else {
        eprintln!("skipped: OPENPLUGIN_PORT_SMOKE_DIR is not set");
        return;
    };
    let mut seen = 0;
    for entry in std::fs::read_dir(dir.join("au")).expect("au folder").flatten() {
        if entry.path().is_dir() {
            for f in std::fs::read_dir(entry.path()).unwrap().flatten() {
                let bytes = std::fs::read(f.path()).unwrap();
                let doc = plist::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", f.path().display()));
                assert!(doc.get("type").is_some() && doc.get("subtype").is_some() && doc.get("manufacturer").is_some());
                seen += 1;
            }
        }
    }
    assert!(seen > 0);
    eprintln!("read {seen} ClassInfo blobs");
}

/// The CLAP preset-discovery path against a provider built for the test (`tests/fixtures/preset_provider.rs`).
#[test]
fn clap_preset_discovery_captures_factory_and_file_presets() {
    let work = out_dir("presets");
    let folder = work.join("presets");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    std::fs::write(folder.join("Gentle.pre"), "0.1\n").unwrap();
    std::fs::write(folder.join("sub").join("Loud.pre"), "0.9").unwrap();
    std::fs::write(folder.join("notes.txt"), "not a preset").unwrap();

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/preset_provider.rs");
    let plugin = work.join("Preset Fixture.clap");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let status = std::process::Command::new(rustc).args(["--edition", "2024", "--crate-type", "cdylib", "-C", "debuginfo=0", "-o"]).arg(&plugin).arg(&source).status().expect("run rustc");
    assert!(status.success(), "building the preset fixture failed");

    let mut o = CaptureOptions::new(helper(), work.join("out"));
    o.env = vec![("OPENPLUGIN_PORT_FIXTURE_PRESETS".into(), folder.display().to_string())];
    let (cap, file) = capture::run(&plugin, &o).expect("capture").remove(0);
    let dir = file.parent().unwrap();
    let got: Vec<(&str, &str)> = cap.states.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect();
    assert_eq!(
        got,
        [("default", "default"), ("Warm", "factory-preset"), ("Bright", "factory-preset"), ("Gentle", "preset"), ("Loud", "preset")],
        "notes: {:?}",
        cap.notes
    );
    let expected = [0.5, 0.25, 0.75, 0.1, 0.9];
    for (state, want) in cap.states.iter().zip(expected) {
        assert_eq!(state.params.len(), 1);
        assert_eq!((state.params[0].id, state.params[0].name.as_str()), (7, "Level"));
        assert!((state.params[0].plain - want).abs() < 1e-12, "{}: {}", state.name, state.params[0].plain);
        // The blob is the stream the plugin wrote: the value as eight little-endian bytes.
        let bytes = std::fs::read(dir.join(&state.blobs[0].file)).unwrap();
        assert_eq!(bytes, want.to_le_bytes());
    }
    assert_eq!(cap.states[1].location.as_deref(), Some("plugin#0"));
    assert!(cap.states[3].location.as_deref().unwrap().ends_with("Gentle.pre"));
    assert!(cap.states[4].location.as_deref().unwrap().ends_with("Loud.pre"));
    // Only presets for this plugin id; the one for another plugin is left out.
    assert!(!cap.states.iter().any(|s| s.name == "Not Mine"));
    // With a limit, the rest are noted, not silently dropped.
    o.max_presets = 2;
    let (cap, _) = capture::run(&plugin, &o).expect("capture").remove(0);
    assert_eq!(cap.states.len(), 3);
    assert!(cap.notes.iter().any(|n| n.contains("--max-presets")), "{:?}", cap.notes);
}
