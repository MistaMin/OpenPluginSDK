//! `openplugin-port new`, end to end: the SDK's own example (`examples/editor`,
//! "Example Gain") is built and scanned as the original, `new` writes a crate
//! from the manifest, and that crate's own `cargo test` builds it, runs its
//! unit tests, and nulls it against the original (the example at its defaults
//! is unity gain, which the generated pass-through matches exactly). Then the
//! generated `process` is changed to halve the signal, and the same null test
//! must fail.
//!
//! Identity: the generated crate keeps the original's ids. The CLAP round trip
//! scans the generated library again and finds the original's CLAP id and
//! parameter ids. The VST3 round trip starts from the pinned example
//! (`examples/pinned`, whose ids are arbitrary numbers, not positions): a VST3
//! manifest of it is made from what its VST3 shell reports (the scanner's
//! VST3 reading is proven against the same shell functions in `tests/scan.rs`),
//! `new` writes a crate, and the generated library's shell exports report the
//! original's VST3 class ids, VST3 parameter ids, Audio Unit codes and AAX
//! codes. With `OPENPLUGIN_PORT_FIXTURES`, a real VST3 scan of the built
//! example is checked the same way.

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
    assert!(params.contains(r#"PluginInfo::new("com.opengrid.example.editor-gain","#), "{params}");
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

    // The port is the same plugin to a CLAP host: scanned again, it has the original's ids.
    let port_lib = target_dir().join("debug").join(format!("{}example_gain{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX));
    let rescanned = scan(&port_lib, &{
        let mut o = ScanOptions::new(tool());
        o.format = Some("clap".into());
        o
    })
    .expect("scan the port")
    .remove(0);
    assert_eq!(rescanned.plugin.ids.clap, manifest.plugin.ids.clap);
    let ids = |m: &Manifest| m.plugin.params.iter().map(|p| (p.id, p.name.clone())).collect::<Vec<_>>();
    assert_eq!(ids(&rescanned), ids(&manifest));

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

// ---------- identity: VST3 class and parameter ids, Audio Unit and AAX codes ----------

use example_pinned::PinnedA;
use openplugin_core::Plugin;
use openplugin_host::shell::{self, AU_MANUFACTURER, AU_SUBTYPE, HOST_VST3, OpShellInfo};
use openplugin_port::manifest::{self as mf, SAMPLE_POINTS, SCHEMA_VERSION, hex16};

/// The ids a plugin's shell exports report: what the VST3, Audio Unit and AAX
/// shells give their hosts.
#[derive(Debug, PartialEq)]
struct ShellIds {
    processor: [u8; 16],
    controller: [u8; 16],
    /// AAX manufacturer, product, mono and stereo plug-in types.
    aax: [u32; 4],
    /// Audio Unit subtype and manufacturer.
    au: [u32; 2],
    /// The VST3 ParamID of each parameter, in table order.
    vst3_params: Vec<u32>,
}

fn shell_ids(info: &OpShellInfo, au: [u32; 2], vst3_param: impl Fn(u32) -> u32) -> ShellIds {
    ShellIds {
        processor: info.processor_cid,
        controller: info.controller_cid,
        aax: [info.manufacturer_id, info.product_id, info.plugin_id_mono, info.plugin_id_stereo],
        au,
        vst3_params: (0..info.param_count).map(vst3_param).collect(),
    }
}

/// The original's, from the same functions its shell exports call.
fn pinned_ids() -> ShellIds {
    let mut info: OpShellInfo = unsafe { std::mem::zeroed() };
    unsafe { shell::write_info::<PinnedA>(&mut info) };
    let au = [shell::au_code::<PinnedA>(AU_SUBTYPE), shell::au_code::<PinnedA>(AU_MANUFACTURER)];
    shell_ids(&info, au, |i| shell::param_host_id::<PinnedA>(HOST_VST3, i))
}

/// A generated crate's, through the exports of its built library.
fn library_ids(lib: &Path) -> ShellIds {
    use openplugin_port::sys::{open_library, symbol};
    let handle = open_library(lib).expect("load the port's library");
    let get = |name: &str| {
        let f = symbol(handle, name);
        assert!(!f.is_null(), "{name} is not exported");
        f
    };
    unsafe {
        let info_fn: unsafe extern "C" fn(*mut OpShellInfo) -> u32 = std::mem::transmute(get("op_shell_info"));
        let host_id: unsafe extern "C" fn(u32, u32) -> u32 = std::mem::transmute(get("op_shell_param_host_id"));
        let au_code: unsafe extern "C" fn(u32) -> u32 = std::mem::transmute(get("op_shell_au_code"));
        let mut info: OpShellInfo = std::mem::zeroed();
        info_fn(&mut info);
        shell_ids(&info, [au_code(AU_SUBTYPE), au_code(AU_MANUFACTURER)], |i| host_id(HOST_VST3, i))
    }
}

fn four(code: u32) -> String {
    String::from_utf8(code.to_be_bytes().to_vec()).unwrap()
}

/// The manifest `scan` writes for the pinned example's VST3: its class ids,
/// its parameters by VST3 ParamID, and the Audio Unit and AAX codes its other
/// bundles carry.
fn pinned_vst3_manifest(ids: &ShellIds, dir: &Path) -> Manifest {
    let plugin = PinnedA::default();
    let info = plugin.info();
    let mut m = Manifest { schema_version: SCHEMA_VERSION, tool: openplugin_port::tool_id(), ..Manifest::default() };
    m.source = mf::Source { format: "vst3".into(), path: dir.join("Pinned.vst3").display().to_string(), index: 0, count: 1, bundle_id: None };
    m.plugin.name = info.name.into();
    m.plugin.vendor = info.vendor.into();
    m.plugin.version = info.version.into();
    m.plugin.categories = vec!["Fx".into()];
    m.plugin.ids.vst3 = Some(mf::Vst3Ids { class_id: hex16(&ids.processor), controller_class_id: Some(hex16(&ids.controller)), category: "Audio Module Class".into(), ..Default::default() });
    m.plugin.ids.au = Some(mf::AuIds { component_type: "aufx".into(), subtype: four(ids.au[0]), manufacturer: four(ids.au[1]), factory_function: None, version: None });
    m.plugin.ids.aax = Some(mf::AaxIds { manufacturer: four(ids.aax[0]), product: four(ids.aax[1]), plugin_ids: vec![four(ids.aax[2]), four(ids.aax[3])], found_in: "page-table".into() });
    for direction in ["input", "output"] {
        m.plugin.buses.push(mf::Bus { direction: direction.into(), kind: "main".into(), name: "Main".into(), channels: 2, arrangement: Some("0x3".into()), layout: Some("stereo".into()) });
    }
    for (p, id) in info.params.iter().zip(&ids.vst3_params) {
        let toggle = p.unit == openplugin_core::Unit::Toggle;
        let (min, max) = (f64::from(p.min), f64::from(p.max));
        m.plugin.params.push(mf::Param {
            id: *id,
            name: p.name.into(),
            min,
            max,
            default: f64::from(p.default),
            default_normalized: (f64::from(p.default) - min) / (max - min),
            step_count: u32::from(toggle),
            flags: vec!["automatable".into()],
            samples: SAMPLE_POINTS.iter().map(|&t| mf::Sample { normalized: t, plain: min + (max - min) * t, text: String::new() }).collect(),
            step_texts: if toggle { vec!["Off".into(), "On".into()] } else { Vec::new() },
            ..Default::default()
        });
    }
    m
}

/// `new` on a manifest, then the generated crate's unit tests and library build. Returns the library.
fn generate_and_build(dir: &Path, manifest: &Manifest, name: &str) -> (PathBuf, String) {
    let json = dir.join(format!("{name}.manifest.json"));
    std::fs::write(&json, serde_json::to_string_pretty(manifest).unwrap()).unwrap();
    let krate = dir.join(name);
    let out = Command::new(tool()).arg("new").arg("--from").arg(&json).arg("--out").arg(&krate).arg("--name").arg(name).arg("--sdk").arg(root()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    std::fs::copy(root().join("Cargo.lock"), krate.join("Cargo.lock")).unwrap();
    // The unit tests only: the original of a made-up manifest is not on disk for the null test.
    let (ok, text) = cargo_test(&krate, &["--lib"]);
    assert!(ok, "{text}");
    assert!(text.contains("every_pin_is_usable ... ok"), "{text}");
    let built = cargo().arg("build").arg("--lib").arg("--manifest-path").arg(krate.join("Cargo.toml")).env("CARGO_TARGET_DIR", target_dir()).status().expect("run cargo");
    assert!(built.success(), "building the port's library failed");
    let lib = target_dir().join("debug").join(format!("{}{}{}", std::env::consts::DLL_PREFIX, name.replace('-', "_"), std::env::consts::DLL_SUFFIX));
    assert!(lib.is_file(), "{} was not built", lib.display());
    (lib, std::fs::read_to_string(krate.join("src/lib.rs")).unwrap() + &std::fs::read_to_string(krate.join("src/params.rs")).unwrap())
}

#[test]
fn the_generated_crate_keeps_the_originals_vst3_audio_unit_and_aax_ids() {
    let dir = scratch("identity");
    let original = pinned_ids();
    // The pinned example's ids are not positions: a port that numbered by position would differ.
    assert!(original.vst3_params.iter().enumerate().any(|(i, id)| *id as usize != i), "{original:?}");
    let manifest = pinned_vst3_manifest(&original, &dir);
    let (lib, source) = generate_and_build(&dir, &manifest, "pinned-port");

    // The pins are code, not comments.
    assert!(source.contains(&format!("Vst3Ids::from_hex({:?}, {:?})", hex16(&original.processor), hex16(&original.controller))), "{source}");
    assert!(source.contains(&format!(".with_vst3_id({:#010X})", original.vst3_params[0])), "{source}");
    assert!(source.contains("fn au_ids(&self) -> Option<AuIds> {\n        Some(AuIds { subtype: *b\"PnEx\", manufacturer: *b\"OpTs\" })"), "{source}");
    assert!(!source.contains("TODO(porting task 2)"), "{source}");

    // The port's shell reports what the original's does.
    assert_eq!(library_ids(&lib), original);
}

#[test]
fn a_vst3_scan_of_the_built_example_gives_a_port_with_its_class_ids() {
    let Some(bundle) = std::env::var_os("OPENPLUGIN_PORT_FIXTURES").map(|d| PathBuf::from(d).join("Example Gain.vst3")).filter(|b| b.exists()) else {
        println!("skipped: OPENPLUGIN_PORT_FIXTURES has no Example Gain.vst3");
        return;
    };
    let dir = scratch("identity-vst3-scan");
    let manifest = scan(&bundle, &ScanOptions::new(tool())).expect("scan the VST3").remove(0);
    let vst3 = manifest.plugin.ids.vst3.clone().expect("VST3 ids");
    let (lib, _) = generate_and_build(&dir, &manifest, "scanned-gain-port");
    let port = library_ids(&lib);
    assert_eq!(hex16(&port.processor), vst3.class_id);
    assert_eq!(Some(hex16(&port.controller)), vst3.controller_class_id);
    let scanned: Vec<u32> = manifest.plugin.params.iter().map(|p| p.id).filter(|id| *id != openplugin_core::identity::VST3_BYPASS_PARAM_ID).collect();
    assert_eq!(port.vst3_params, scanned);
}
