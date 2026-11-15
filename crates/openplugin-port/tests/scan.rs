//! The scanner against the SDK's own example (`examples/editor`, "Example Gain").
//!
//! The CLAP scan always runs: the test builds the example's `cdylib` and scans
//! it. The VST3, Audio Unit and AAX scans need bundles built by
//! `scripts/build_plugin.sh` (they need the VST3, AudioUnitSDK and AAX
//! checkouts), so they run when `OPENPLUGIN_PORT_FIXTURES` names the folder
//! that holds `Example Gain.clap`, `.vst3`, `.component` and `.aaxplugin`
//! (see `docs/PORTING.md`) and print "skipped" otherwise.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use example_editor::{EditorGain, INFO};
use openplugin_core::{ParamFlags, ParamInfo, Plugin};
use openplugin_ffi::shell::{OpShellInfo, write_info};
use openplugin_port::manifest::{Manifest, hex16};
use openplugin_port::snippet::{self, Inferred};
use openplugin_port::{ScanOptions, scan};

fn options() -> ScanOptions {
    ScanOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_openplugin-port")))
}

/// The example's cdylib, built once.
fn example_dylib() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo)
            .args(["build", "-p", "example-editor", "--lib"])
            .current_dir(&root)
            .status()
            .expect("run cargo");
        assert!(status.success(), "building the example failed");
        let target = std::env::var_os("CARGO_TARGET_DIR").map(|t| root.join(t)).unwrap_or_else(|| root.join("target"));
        let dylib = target.join("debug").join("libexample_editor.dylib");
        assert!(dylib.is_file(), "{} was not built", dylib.display());
        dylib
    })
}

fn fixtures() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENPLUGIN_PORT_FIXTURES")?);
    dir.is_dir().then_some(dir)
}

fn flag_bits(names: &[&str]) -> u32 {
    names
        .iter()
        .map(|n| match *n {
            "AUTOMATABLE" => ParamFlags::AUTOMATABLE,
            "STEPPED" => ParamFlags::STEPPED,
            "HIDDEN" => ParamFlags::HIDDEN,
            "READ_ONLY" => ParamFlags::READ_ONLY,
            "BYPASS" => ParamFlags::BYPASS,
            "NOT_REALTIME" => ParamFlags::NOT_REALTIME,
            other => panic!("unexpected flag {other}"),
        })
        .fold(0, |acc, f| acc | f.bits())
}

fn same_param(got: &Inferred, want: &ParamInfo) {
    let ctx = format!("parameter {:?}: {got:?}", want.id);
    assert_eq!(got.id, want.id, "{ctx}");
    assert_eq!(got.name, want.name, "{ctx}");
    assert_eq!(got.min, want.min, "{ctx}");
    assert_eq!(got.max, want.max, "{ctx}");
    assert_eq!(got.default, want.default, "{ctx}");
    assert_eq!(got.unit, format!("{:?}", want.unit), "{ctx}");
    assert_eq!(got.taper, format!("{:?}", want.taper), "{ctx}");
    assert_eq!(got.steps, want.steps, "{ctx}");
    assert_eq!(got.choices, want.choices.iter().map(|c| c.to_string()).collect::<Vec<_>>(), "{ctx}");
    assert_eq!(flag_bits(&got.flags), want.flags.bits(), "{ctx}");
}

/// The manifest's parameters, turned into SDK terms, are the example's table. VST3 adds a host bypass parameter of its own.
fn assert_reproduces_table(m: &Manifest) {
    let mut plugin = m.plugin.clone();
    plugin.params.retain(|p| !p.has_flag("bypass"));
    let inferred = snippet::infer(&plugin);
    assert_eq!(inferred.len(), INFO.params.len(), "parameter count");
    for (got, want) in inferred.iter().zip(INFO.params) {
        same_param(got, want);
    }
    // The ids the source reports are positions 0..n, as the SDK numbers them.
    for (i, p) in plugin.params.iter().enumerate() {
        assert_eq!(p.id as usize, i);
    }
    assert_eq!(m.plugin.latency_samples.unwrap_or(0), 0);
}

/// The generated Rust has the example's own lines.
fn assert_snippet_lines(m: &Manifest) {
    let text = snippet::render(m);
    assert!(text.contains("ParamInfo::new(\"gain\", \"Gain\", -24.0, 24.0, 0.0, Unit::Db),"), "{text}");
    assert!(text.contains("ParamInfo::new(\"warm\", \"Warm\", 0.0, 1.0, 0.0, Unit::Toggle),"), "{text}");
}

fn shell_info() -> OpShellInfo {
    let mut info: OpShellInfo = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { write_info::<EditorGain>(&mut info) }, 1);
    info
}

#[test]
fn clap_scan_reproduces_the_example() {
    let manifests = scan(example_dylib(), &{
        let mut o = options();
        o.format = Some("clap".into());
        o
    })
    .expect("scan");
    assert_eq!(manifests.len(), 1);
    let m = &manifests[0];
    assert_eq!(m.schema_version, 1);
    assert_eq!(m.plugin.ids.clap.as_ref().expect("clap id").id, INFO.id);
    assert_eq!(m.plugin.name, INFO.name);
    assert_eq!(m.plugin.vendor, INFO.vendor);
    assert_reproduces_table(m);
    assert_snippet_lines(m);
    // The example is a stereo effect.
    assert!(m.plugin.buses.iter().any(|b| b.direction == "input" && b.channels == 2));
    assert!(m.plugin.buses.iter().any(|b| b.direction == "output" && b.channels == 2));
    // The manifest survives a JSON round trip unchanged.
    let json = serde_json::to_string_pretty(m).unwrap();
    let back: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, m);
}

#[test]
fn clap_scan_writes_the_files() {
    let manifests = scan(example_dylib(), &{
        let mut o = options();
        o.format = Some("clap".into());
        o
    })
    .unwrap();
    // Next to the test binary, inside the build folder.
    let out = std::env::current_exe().unwrap().parent().unwrap().join("port-test-out");
    let files = openplugin_port::write_outputs(&manifests, &out).unwrap();
    assert_eq!(files.len(), 2);
    assert!(std::fs::read_to_string(&files[0]).unwrap().contains("\"schema_version\": 1"));
    assert!(std::fs::read_to_string(&files[1]).unwrap().contains("pub static PARAMS: [ParamInfo; 2]"));
}

#[test]
fn a_crashing_plugin_fails_one_scan_and_nothing_else() {
    let mut o = options();
    o.format = Some("clap".into());
    o.env = vec![(openplugin_port::probe::FAULT_ENV.into(), "crash".into())];
    let err = scan(example_dylib(), &o).unwrap_err();
    assert!(err.contains("crashed"), "{err}");
    // The scanner is still here and the next scan works.
    let mut ok = options();
    ok.format = Some("clap".into());
    assert!(scan(example_dylib(), &ok).is_ok());
}

#[test]
fn a_hanging_plugin_is_killed_at_the_timeout() {
    let mut o = options();
    o.format = Some("clap".into());
    o.timeout = Duration::from_millis(600);
    o.env = vec![(openplugin_port::probe::FAULT_ENV.into(), "hang".into())];
    let start = std::time::Instant::now();
    let err = scan(example_dylib(), &o).unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    // A hang would never return; the bound is loose so a loaded machine starting the
    // child process slowly does not fail the test.
    assert!(start.elapsed() < Duration::from_secs(60));
}

#[test]
fn things_that_are_not_plugins_are_errors() {
    let dir = std::env::current_exe().unwrap().parent().unwrap().join("port-not-a-plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("empty.clap");
    std::fs::write(&file, b"not a library").unwrap();
    assert!(scan(&file, &options()).is_err());
    assert!(scan(&dir.join("missing.vst3"), &options()).is_err());
    assert!(scan(&file, &{
        let mut o = options();
        o.format = Some("nope".into());
        o
    })
    .is_err());
}

#[test]
fn vst3_scan_reproduces_the_example() {
    let Some(dir) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    let manifests = scan(&dir.join("Example Gain.vst3"), &options()).expect("scan");
    let m = &manifests[0];
    assert_reproduces_table(m);
    assert_snippet_lines(m);
    let info = shell_info();
    let v = m.plugin.ids.vst3.as_ref().expect("vst3 ids");
    assert_eq!(v.class_id, hex16(&info.processor_cid));
    assert_eq!(v.controller_class_id.as_deref(), Some(hex16(&info.controller_cid).as_str()));
    assert!(!v.single_component);
    // The host bypass parameter is reported and flagged.
    assert!(m.plugin.params.iter().any(|p| p.has_flag("bypass")));
}

#[test]
fn au_scan_reproduces_the_example() {
    let Some(dir) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    let manifests = scan(&dir.join("Example Gain.component"), &options()).expect("scan");
    let m = &manifests[0];
    assert_reproduces_table(m);
    assert_snippet_lines(m);
    let au = m.plugin.ids.au.as_ref().expect("au ids");
    assert_eq!((au.component_type.as_str(), au.subtype.as_str(), au.manufacturer.as_str()), ("aufx", "ExGn", "OpGd"));
    assert_eq!(au.factory_function.as_deref(), Some("OpenPluginAUFactory"));
}

#[test]
fn aax_scan_reads_the_page_table_ids() {
    let Some(dir) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    let manifests = scan(&dir.join("Example Gain.aaxplugin"), &options()).expect("scan");
    let aax = manifests[0].plugin.ids.aax.as_ref().expect("aax ids");
    let want = EditorGain::default().aax_ids().expect("the example sets AAX ids");
    let text = |c: [u8; 4]| String::from_utf8(c.to_vec()).unwrap();
    assert_eq!(aax.manufacturer, text(want.manufacturer));
    assert_eq!(aax.product, text(want.product));
    assert_eq!(aax.plugin_ids, vec![text(want.mono), text(want.stereo)]);
}

#[test]
fn clap_bundle_scan_matches_the_bare_library() {
    let Some(dir) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    let manifests = scan(&dir.join("Example Gain.clap"), &options()).expect("scan");
    assert_reproduces_table(&manifests[0]);
    assert_eq!(manifests[0].source.bundle_id.as_deref(), Some("com.opengrid.example"));
}
