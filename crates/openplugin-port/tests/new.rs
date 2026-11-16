//! `openplugin-port new`, end to end: the SDK's own example (`examples/editor`,
//! "Example Gain") is built and scanned as the original, `new` writes a crate
//! from the manifest, and that crate's own `cargo test` builds it, runs its
//! unit tests, and nulls it against the original (the example at its defaults
//! is unity gain, which the generated pass-through matches exactly). Then the
//! generated `process` is changed to halve the signal, and the same null test
//! must fail.

use std::path::{Path, PathBuf};
use std::process::Command;

use openplugin_port::manifest::Manifest;
use openplugin_port::scaffold::{self, NewOptions, SdkSource};
use openplugin_port::{ScanOptions, scan, write_outputs};

fn tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_openplugin-port"))
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn target_dir() -> PathBuf {
    // A relative CARGO_TARGET_DIR is relative to the workspace root.
    std::env::var_os("CARGO_TARGET_DIR").map(|t| root().join(t)).unwrap_or_else(|| root().join("target"))
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::current_exe().unwrap().parent().unwrap().join("port-new-test").join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The example's cdylib, built, copied as `original.clap` into `dir`.
fn original(dir: &Path) -> PathBuf {
    let status = cargo().args(["build", "-p", "example-editor", "--lib"]).current_dir(root()).status().expect("run cargo");
    assert!(status.success(), "building the example failed");
    let dylib = target_dir().join("debug").join(format!("{}example_editor{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX));
    let dst = dir.join("original.clap");
    std::fs::copy(&dylib, &dst).unwrap();
    dst
}

fn scan_original(dir: &Path) -> (Manifest, PathBuf) {
    let clap = original(dir);
    let manifests = scan(&clap, &ScanOptions::new(tool())).expect("scan the example");
    let files = write_outputs(&manifests, dir).unwrap();
    let json = files.into_iter().find(|f| f.to_string_lossy().ends_with(".manifest.json")).unwrap();
    (manifests.into_iter().next().unwrap(), json)
}

/// `cargo test` in the generated crate, sharing the workspace's target folder
/// (so the SDK is not compiled twice) and its lock file (so the versions are
/// the ones the workspace is tested with).
fn cargo_test(krate: &Path, args: &[&str]) -> (bool, String) {
    let out = cargo()
        .arg("test")
        .arg("--manifest-path")
        .arg(krate.join("Cargo.toml"))
        .args(args)
        .env("CARGO_TARGET_DIR", target_dir())
        .env_remove("OPENPLUGIN_PORT")
        .env_remove("OPENPLUGIN_PORT_ORIGINAL")
        .output()
        .expect("run cargo");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn scan_then_new_gives_a_crate_that_builds_and_nulls_against_the_original() {
    let dir = scratch("round-trip");
    let (manifest, json) = scan_original(&dir);
    let krate = dir.join("example-gain");

    // The CLI, as a user runs it.
    let out = Command::new(tool()).arg("new").arg("--from").arg(&json).arg("--out").arg(&krate).arg("--sdk").arg(root()).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("2 parameters"), "{text}");

    // What it wrote is what the library generates.
    let opts = NewOptions { sdk: SdkSource::Path(root()), crate_name: None, tool: tool() };
    for f in scaffold::generate(&manifest, &opts).unwrap() {
        assert_eq!(std::fs::read_to_string(krate.join(f.path)).unwrap(), f.text, "{}", f.path);
    }
    let read = |p: &str| std::fs::read_to_string(krate.join(p)).unwrap();
    assert!(read("Cargo.toml").contains("name = \"example-gain\""));
    let params = read("src/params.rs");
    assert!(params.contains(r#"ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),"#), "{params}");
    assert!(params.contains(r#"id: "com.opengrid.example.editor-gain","#), "{params}");
    let lib = read("src/lib.rs");
    assert!(lib.contains("pub struct ExampleGainPort"));
    assert!(lib.contains("TODO(porting): no AAX ids in the scan"), "a CLAP scan has no AAX ids: {lib}");
    assert!(lib.contains("fn supported_layouts(&self) -> &'static [IoConfig] {\n        &[IoConfig { input: SpeakerLayout::Stereo, output: SpeakerLayout::Stereo, sidechain: None }]"), "{lib}");
    let import = read("src/state_import.rs");
    assert!(import.contains(r#"m.map(ForeignId::Index(0), "gain"); // "Gain""#), "{import}");
    assert!(import.contains(r#"m.map(ForeignId::Index(1), "warm"); // "Warm""#), "{import}");
    assert!(read("tests/null.rs").contains(&format!("const ORIGINAL: &str = {:?};", manifest.source.path)));

    // A second `new` into the same folder refuses and changes nothing.
    let again = Command::new(tool()).arg("new").arg("--from").arg(&json).arg("--out").arg(&krate).output().unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("is not empty"));
    assert_eq!(read("src/lib.rs"), lib);

    // The generated crate builds, its unit tests pass, and its null test passes against the original.
    std::fs::copy(root().join("Cargo.lock"), krate.join("Cargo.lock")).unwrap();
    let (ok, text) = cargo_test(&krate, &["--", "--nocapture"]);
    assert!(ok, "{text}");
    for name in ["own_state_round_trips", "a_refused_state_changes_nothing", "every_parameter_of_the_original_has_a_place", "the_port_nulls_against_the_original"] {
        assert!(text.contains(&format!("{name} ... ok")), "{name} did not run and pass:\n{text}");
    }
    assert!(text.contains("PASS"), "the null report: {text}");
    assert!(text.contains("max |diff| 0"), "an exact null: {text}");

    // Negative control: a port that halves the signal fails the same null test.
    let changed = lib.replace("let _ = (io, frames);", "for ch in io.iter_mut() {\n            for s in &mut ch[..frames] {\n                *s *= 0.5;\n            }\n        }");
    assert_ne!(changed, lib);
    std::fs::write(krate.join("src/lib.rs"), changed).unwrap();
    let (ok, text) = cargo_test(&krate, &["--test", "null"]);
    assert!(!ok, "a port at half gain must not null:\n{text}");
    assert!(text.contains("the port does not null against the original"), "{text}");
}

#[test]
fn new_refuses_what_it_cannot_read() {
    let dir = scratch("refusals");
    let run = |args: &[&str]| Command::new(tool()).arg("new").args(args).output().unwrap();

    let toml = dir.join("plugin.manifest.toml");
    std::fs::write(&toml, "schema_version = 1\n").unwrap();
    let out = run(&["--from", toml.to_str().unwrap(), "--out", dir.join("a").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("the scanner writes JSON"));

    let future = dir.join("future.manifest.json");
    std::fs::write(&future, r#"{"schema_version": 99}"#).unwrap();
    let out = run(&["--from", future.to_str().unwrap(), "--out", dir.join("b").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("manifest schema 99"));

    assert_eq!(run(&[]).status.code(), Some(2), "no --from is a usage error");
    assert!(!dir.join("a").exists() && !dir.join("b").exists());
}
