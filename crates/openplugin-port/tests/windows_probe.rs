//! The probe child on Windows, against real DLLs (Windows only).
//!
//! - Faults raised inside a plug-in's own code (`tests/fixtures/windows_faults.rs`, built with rustc):
//!   each structured exception is named with its code, a hang is killed at the timeout, and the
//!   killed child has let go of the DLL.
//! - Files that are not CLAP plug-ins fail with the system's reason.
//! - A Windows `.clap` (the DLL under another name) in a folder whose name has a space and a
//!   non-ASCII letter scans, captures and renders like the bare library.
//!
//! VST3 is not here: loading a Windows `.vst3` needs the Windows VST3 shell (`native/vst3/win/`).
//! The fixture-based VST3, Audio Unit and AAX tests in `scan.rs`, `null.rs` and `state_import.rs`
//! are ignored on Windows with that reason.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use example_editor::INFO;
use openplugin_port::capture::{self, CaptureOptions};
use openplugin_port::null::{NullOptions, run_null};
use openplugin_port::{ScanOptions, scan};

const FAULT: &str = "OPENPLUGIN_PORT_FIXTURE_FAULT";

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_openplugin-port"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::current_exe().unwrap().parent().unwrap().join("port-windows-test").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The fault fixture, built once.
fn fault_fixture() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/windows_faults.rs");
        let out = scratch("fixture").join("Fault Fixture.dll");
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let status = std::process::Command::new(rustc)
            // The static C runtime, as every Windows binary of the SDK (.cargo/config.toml does not
            // reach a direct rustc call); CI fails on a vcruntime140.dll import.
            .args(["--edition", "2024", "--crate-type", "cdylib", "-C", "debuginfo=0", "-C", "target-feature=+crt-static", "-o"])
            .arg(&out)
            .arg(&source)
            .status()
            .expect("run rustc");
        assert!(status.success(), "building the fault fixture failed");
        out
    })
}

/// A private copy of the fixture as `<name>.clap`, so a test can delete it afterwards.
fn fault_copy(name: &str) -> PathBuf {
    let p = scratch(name).join(format!("{name}.clap"));
    std::fs::copy(fault_fixture(), &p).unwrap();
    p
}

/// The example's DLL, built once.
fn example_dll() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo).args(["build", "-p", "example-editor", "--lib"]).current_dir(&root).status().expect("run cargo");
        assert!(status.success(), "building the example failed");
        let target = std::env::var_os("CARGO_TARGET_DIR").map(|t| root.join(t)).unwrap_or_else(|| root.join("target"));
        let dll = target.join("debug").join("example_editor.dll");
        assert!(dll.is_file(), "{} was not built", dll.display());
        dll
    })
}

fn scan_clap(path: &Path, fault: Option<&str>, timeout: Duration) -> Result<(), String> {
    let mut o = ScanOptions::new(helper());
    o.format = Some("clap".into());
    o.timeout = timeout;
    if let Some(f) = fault {
        o.env = vec![(FAULT.into(), f.into())];
    }
    scan(path, &o).map(|_| ())
}

#[test]
fn each_exception_in_a_plugin_is_named_with_its_code() {
    let plugin = fault_copy("exceptions");
    for (fault, name, code) in [
        ("access", "access violation", "0xC0000005"),
        ("illegal", "illegal instruction", "0xC000001D"),
        ("stack", "stack overflow", "0xC00000FD"),
        ("panic", "fast fail", "0xC0000409"),
    ] {
        let err = scan_clap(&plugin, Some(fault), Duration::from_secs(60)).unwrap_err();
        assert!(err.contains("crashed") && err.contains(name) && err.contains(code), "{fault}: {err}");
    }
    // Without a fault the fixture's init refuses, and the scan says so.
    let err = scan_clap(&plugin, None, Duration::from_secs(60)).unwrap_err();
    assert!(err.contains("init returned false"), "{err}");
}

#[test]
fn a_plugin_that_hangs_is_killed_and_lets_go_of_its_dll() {
    let plugin = fault_copy("hang");
    let start = Instant::now();
    let err = scan_clap(&plugin, Some("hang"), Duration::from_millis(800)).unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(60));
    // Windows keeps a loaded DLL's file open. The child was killed and waited for, so the file can go.
    std::fs::remove_file(&plugin).unwrap_or_else(|e| panic!("the killed probe still holds {}: {e}", plugin.display()));
}

#[test]
fn files_that_are_not_clap_plugins_fail_with_the_system_reason() {
    let dir = scratch("not-plugins");
    let junk = dir.join("junk.clap");
    std::fs::write(&junk, b"not a library").unwrap();
    let err = scan_clap(&junk, None, Duration::from_secs(60)).unwrap_err();
    // ERROR_BAD_EXE_FORMAT.
    assert!(err.contains("error 193"), "{err}");

    // A real DLL without `clap_entry`.
    let system = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32").join("version.dll");
    let no_entry = dir.join("no entry.clap");
    std::fs::copy(&system, &no_entry).unwrap();
    let err = scan_clap(&no_entry, None, Duration::from_secs(60)).unwrap_err();
    assert!(err.contains("no clap_entry symbol"), "{err}");
}

/// The Windows form of a `.clap`, in a folder with a space and a non-ASCII letter: the DLL is
/// loaded by its wide path, and the plug-in gets the path as UTF-8 in `clap_entry.init`.
#[test]
fn a_windows_clap_file_scans_captures_and_nulls() {
    let dir = scratch("Plug-ins \u{00fc}");
    let a = dir.join("Example Gain.clap");
    let b = dir.join("Example Gain copy.clap");
    std::fs::copy(example_dll(), &a).unwrap();
    std::fs::copy(example_dll(), &b).unwrap();

    let manifests = scan(&a, &ScanOptions::new(helper())).expect("scan");
    assert_eq!(manifests.len(), 1);
    let m = &manifests[0];
    assert_eq!(m.source.format, "clap");
    assert_eq!(m.source.bundle_id, None, "a Windows .clap has no Info.plist");
    assert_eq!(m.plugin.name, INFO.name);
    assert_eq!(m.plugin.ids.clap.as_ref().expect("clap id").id, INFO.id);
    assert_eq!(m.plugin.params.len(), 2);

    let out = dir.join("capture");
    let caps = capture::run(&a, &CaptureOptions::new(helper(), out)).expect("capture");
    assert_eq!(caps.len(), 1);
    assert!(!caps[0].0.states.is_empty() && !caps[0].0.states[0].blobs.is_empty());

    let mut o = NullOptions::new(helper(), a.clone(), b.clone());
    o.input = "noise:0.5".into();
    o.automation = Some("0:Gain=-3, 900:Gain=6".into());
    o.timeout = Duration::from_secs(60);
    let r = run_null(&o).expect("null");
    assert!(r.pass, "{}", r.to_text());
    assert!(r.channels.iter().all(|c| c.max_abs_diff == 0.0));
}
