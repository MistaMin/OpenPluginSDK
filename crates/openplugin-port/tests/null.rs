//! The null test against the SDK's own example (`examples/editor`).
//!
//! CLAP always runs: the test builds the example's `cdylib`, copies it, and
//! nulls the two. VST3 and Audio Unit run when `OPENPLUGIN_PORT_FIXTURES`
//! names a folder holding `Example Gain.vst3` and `Example Gain.component`
//! (built by `scripts/build_plugin.sh`, see `docs/PORTING.md`); the latency
//! tests also want `Latency Stub.vst3` and `LatencyTest.component` there (the
//! test bundles of `native/CMakeLists.txt`). Without them those tests print
//! "skipped".

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use openplugin_port::null::{NullOptions, NullReport, run_null};
use openplugin_port::probe::FAULT_ENV;
use openplugin_port::render::GAIN_ENV;
use openplugin_port::wav::{self, Audio};

const AUTOMATION: &str = "0:Gain=-3, 700:Gain=6, 1800:Warm=1, 2048:Gain=2, 2600:Gain=-9.5, 3300:Gain=12, 3300:Warm=0";

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_openplugin-port"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::current_exe().unwrap().parent().unwrap().join("port-null-test").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn example_dylib() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo).args(["build", "-p", "example-editor", "--lib"]).current_dir(&root).status().expect("run cargo");
        assert!(status.success(), "building the example failed");
        let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
        let dylib = target.join("debug").join("libexample_editor.dylib");
        assert!(dylib.is_file(), "{} was not built", dylib.display());
        dylib
    })
}

/// The same code, "built twice": a second copy of the file at another path.
fn clap_pair() -> (PathBuf, PathBuf) {
    let dir = scratch("clap-pair");
    let a = dir.join("original.clap");
    let b = dir.join("port.clap");
    std::fs::copy(example_dylib(), &a).unwrap();
    std::fs::copy(example_dylib(), &b).unwrap();
    (a, b)
}

fn fixtures() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENPLUGIN_PORT_FIXTURES")?);
    dir.is_dir().then_some(dir)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).unwrap();
        }
    }
}

fn opts(original: &Path, port: &Path, format: Option<&str>) -> NullOptions {
    let mut o = NullOptions::new(helper(), original.to_path_buf(), port.to_path_buf());
    o.original_format = format.map(str::to_string);
    o.port_format = format.map(str::to_string);
    o.timeout = Duration::from_secs(60);
    o
}

fn assert_null(r: &NullReport) {
    assert!(r.pass, "{}", r.to_text());
    assert_eq!(r.channels.len(), 2);
    for c in &r.channels {
        assert_eq!(c.max_abs_diff, 0.0, "{}", r.to_text());
        assert!(c.first_differing.is_none() && c.rms_diff_db.is_none());
    }
    assert!(!r.latency_differs);
}

/// Identical builds null across every input, and the render is not silence: it follows the automation.
fn identical_nulls(original: &Path, port: &Path, format: Option<&str>, tag: &str) {
    for input in ["sweep:1", "noise:1", "impulses:1", "burst:1", "silence:0.2"] {
        let mut o = opts(original, port, format);
        o.input = input.into();
        o.automation = Some(AUTOMATION.into());
        o.save_dir = Some(scratch(&format!("{tag}-{}", input.split(':').next().unwrap())));
        let r = run_null(&o).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_null(&r);
        assert_eq!(r.rate, 48000);
        assert_eq!(r.parameter_map.len(), 2, "{:?}", r.parameter_map);
        if input.starts_with("noise") {
            let dir = o.save_dir.unwrap();
            let i = wav::read(&dir.join("input.wav")).unwrap();
            let out = wav::read(&dir.join("original.wav")).unwrap();
            // Gain -3 dB at the start, +6 dB after frame 700 (warm still off until 1800).
            let near = |a: f32, b: f32| (a - b).abs() < 1e-4 * b.abs().max(1e-3);
            assert!(near(out.channels[0][100], i.channels[0][100] * 10f32.powf(-3.0 / 20.0)));
            assert!(near(out.channels[0][1000], i.channels[0][1000] * 10f32.powf(6.0 / 20.0)));
        }
    }
}

#[test]
fn clap_identical_builds_null() {
    let (a, b) = clap_pair();
    identical_nulls(&a, &b, Some("clap"), "clap");
}

#[test]
fn clap_changed_port_fails_with_the_first_differing_sample() {
    let (a, b) = clap_pair();
    let mut o = opts(&a, &b, Some("clap"));
    o.input = "noise:1".into();
    o.automation = Some(AUTOMATION.into());
    o.save_dir = Some(scratch("clap-changed"));
    // The port's child multiplies its output by 1.001 (a test-only hook of the render host).
    o.env_port = vec![(GAIN_ENV.into(), "1.001".into())];
    let r = run_null(&o).unwrap();
    assert!(!r.pass);
    let c = &r.channels[0];
    let first = c.first_over_tolerance.as_ref().expect("a first sample over tolerance");
    assert_eq!(first.index, 0, "{}", r.to_text());
    assert_eq!(first.port, first.original * 1.001);
    assert!(c.samples_over_tolerance > 1000);
    assert!(c.max_abs_diff > 1e-4 && c.rms_diff_db.unwrap() < -40.0);
    assert!(r.to_text().contains("first sample over tolerance: 0 original"));
    // A tolerance above the difference passes it again.
    o.tolerance = 0.5;
    assert!(run_null(&o).unwrap().pass);
    // The difference file holds exactly the difference.
    let dir = scratch("clap-changed");
    let orig = wav::read(&dir.join("original.wav")).unwrap();
    let diff = wav::read(&dir.join("difference.wav")).unwrap();
    let port = wav::read(&dir.join("port.wav")).unwrap();
    assert_eq!(diff.channels[0][5], orig.channels[0][5] - port.channels[0][5]);
}

#[test]
fn a_crash_or_hang_in_one_plugin_fails_cleanly() {
    let (a, b) = clap_pair();
    let mut o = opts(&a, &b, Some("clap"));
    o.input = "noise:0.5".into();
    o.env_port = vec![(FAULT_ENV.into(), "crash-render".into())];
    let err = run_null(&o).unwrap_err();
    assert!(err.contains("port") && err.contains("crashed"), "{err}");
    let mut o = opts(&a, &b, Some("clap"));
    o.env_original = vec![(FAULT_ENV.into(), "crash".into())];
    let err = run_null(&o).unwrap_err();
    assert!(err.contains("crashed"), "{err}");

    let mut o = opts(&a, &b, Some("clap"));
    o.env_port = vec![(FAULT_ENV.into(), "hang-render".into())];
    o.timeout = Duration::from_secs(3);
    let start = std::time::Instant::now();
    let err = run_null(&o).unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(30));
    // And the next run is fine.
    assert!(run_null(&opts(&a, &b, Some("clap"))).unwrap().pass);
}

#[test]
fn wav_input_in_every_sample_format() {
    let (a, b) = clap_pair();
    let dir = scratch("wav-in");
    // A 997 Hz tone, written as 16-bit, 24-bit and 32-bit int and 32-bit float.
    let n = 6000;
    let tone: Vec<f32> = (0..n).map(|i| 0.4 * (i as f32 * 0.13).sin()).collect();
    for (bits, tag) in [(16u16, 1u16), (24, 1), (32, 1), (32, 3)] {
        let mut data = Vec::new();
        for s in &tone {
            for _ in 0..2 {
                match (bits, tag) {
                    (16, _) => data.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes()),
                    (24, _) => data.extend_from_slice(&((s * 8_388_607.0) as i32).to_le_bytes()[..3]),
                    (32, 1) => data.extend_from_slice(&((f64::from(*s) * 2_147_483_647.0) as i32).to_le_bytes()),
                    _ => data.extend_from_slice(&s.to_le_bytes()),
                }
            }
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&48000u32.to_le_bytes());
        f.extend_from_slice(&(48000 * 2 * u32::from(bits) / 8).to_le_bytes());
        f.extend_from_slice(&(2 * bits / 8).to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        let path = dir.join(format!("tone-{bits}-{tag}.wav"));
        std::fs::write(&path, f).unwrap();
        let mut o = opts(&a, &b, Some("clap"));
        o.input = path.display().to_string();
        let r = run_null(&o).unwrap();
        assert_null(&r);
        assert_eq!(r.frames, n);
    }
    // A rate that disagrees with the file is refused rather than resampled.
    let mut o = opts(&a, &b, Some("clap"));
    o.input = dir.join("tone-16-1.wav").display().to_string();
    o.rate = Some(44100);
    assert!(run_null(&o).unwrap_err().contains("44100"));
}

#[test]
fn param_map_connects_differently_named_parameters() {
    let (a, b) = clap_pair();
    let dir = scratch("param-map");
    // The original's "Gain" drives the port's "Warm" (a real difference), so the null fails.
    let map = dir.join("map.json");
    std::fs::write(&map, r#"{"Gain": "Warm"}"#).unwrap();
    let mut o = opts(&a, &b, Some("clap"));
    o.input = "noise:0.5".into();
    o.automation = Some("0:Gain=6".into());
    o.param_map = Some(map.clone());
    let r = run_null(&o).unwrap();
    assert!(!r.pass);
    assert_eq!(r.parameter_map, vec!["0 (Gain) -> 1 (Warm)".to_string()]);
    // An unknown parameter is an error that says which.
    o.automation = Some("0:Nope=1".into());
    assert!(run_null(&o).unwrap_err().contains("Nope"));
    // The generated map (same name, else same id) is the identity here.
    let out = std::process::Command::new(helper()).args(["param-map"]).arg(&a).arg(&b).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v, serde_json::json!({"0": 0, "1": 1}));
}

#[test]
fn cli_exit_codes() {
    let (a, b) = clap_pair();
    let dir = scratch("cli");
    let run = |args: &[&str]| {
        std::process::Command::new(helper())
            .args(["null"])
            .arg(&a)
            .arg(&b)
            .args(["--format", "clap", "--input", "noise:0.3", "--save"])
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };
    let ok = run(&["--automation", "0:Gain=-3,500:Warm=1", "--block", "100", "--rate", "48000", "--json", dir.join("r.json").to_str().unwrap()]);
    assert_eq!(ok.status.code(), Some(0), "{}", String::from_utf8_lossy(&ok.stdout));
    let text = String::from_utf8_lossy(&ok.stdout);
    assert!(text.contains("PASS") && text.contains("RMS diff -inf dB"), "{text}");
    for f in ["input.wav", "original.wav", "port.wav", "difference.wav"] {
        assert!(dir.join(f).is_file());
    }
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("r.json")).unwrap()).unwrap();
    assert_eq!(json["pass"], true);
    // A map that sends Gain to Warm makes the port differ: exit 1.
    let map = dir.join("map.json");
    std::fs::write(&map, r#"{"0": 1}"#).unwrap();
    let bad = run(&["--automation", "0:Gain=6", "--param-map", map.to_str().unwrap()]);
    assert_eq!(bad.status.code(), Some(1), "{}", String::from_utf8_lossy(&bad.stdout));
    assert!(String::from_utf8_lossy(&bad.stdout).contains("FAIL"));
    // A missing port: exit 3.
    let missing = std::process::Command::new(helper()).args(["null"]).arg(&a).arg(dir.join("missing.clap")).args(["--format", "clap"]).output().unwrap();
    assert_eq!(missing.status.code(), Some(3));
    // Bad usage: exit 2.
    assert_eq!(std::process::Command::new(helper()).args(["null", "--bogus"]).status().unwrap().code(), Some(2));
}

// ---------- VST3 and Audio Unit (fixtures) ----------

fn formats() -> [(&'static str, &'static str); 2] {
    [("Example Gain.vst3", "vst3"), ("Example Gain.component", "au")]
}

#[test]
fn vst3_and_au_identical_builds_null_and_a_changed_port_fails() {
    let Some(dir) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    for (bundle, tag) in formats() {
        let original = dir.join(bundle);
        if !original.exists() {
            eprintln!("skipped: {bundle} is not in the fixtures");
            continue;
        }
        let port = scratch(&format!("{tag}-copy")).join(bundle);
        copy_dir(&original, &port);
        identical_nulls(&original, &port, None, tag);

        let mut o = opts(&original, &port, None);
        o.input = "noise:1".into();
        o.automation = Some(AUTOMATION.into());
        o.env_port = vec![(GAIN_ENV.into(), "1.001".into())];
        let r = run_null(&o).unwrap();
        assert!(!r.pass, "{tag}");
        let first = r.channels[0].first_over_tolerance.as_ref().unwrap();
        assert_eq!(first.index, 0);
        assert_eq!(first.port, first.original * 1.001);

        // A crash in the port's render fails cleanly.
        let mut o = opts(&original, &port, None);
        o.env_port = vec![(FAULT_ENV.into(), "crash-render".into())];
        assert!(run_null(&o).unwrap_err().contains("crashed"), "{tag}");
    }
}

/// A plugin that reports 37 samples of latency is aligned by it: its render is shifted by the
/// latency, and the report says the latencies differ.
#[test]
fn latency_is_reported_and_aligned() {
    let Some(dir) = fixtures() else {
        eprintln!("skipped: OPENPLUGIN_PORT_FIXTURES is not set");
        return;
    };
    for (original, port, tag) in [("Example Gain.vst3", "Latency Stub.vst3", "vst3"), ("Example Gain.component", "LatencyTest.component", "au")] {
        let (original, port) = (dir.join(original), dir.join(port));
        if !original.exists() || !port.exists() {
            eprintln!("skipped: the latency bundles are not in the fixtures");
            continue;
        }
        // Same plugin twice: equal latencies, nulls.
        let mut o = opts(&port, &port, None);
        o.input = "noise:0.5".into();
        let r = run_null(&o).unwrap();
        assert_null_with_latency(&r, 37);

        // The example (latency 0) against the stub (37, gain 0.5 or 1; the stub does not delay).
        let mut o = opts(&original, &port, None);
        o.input = "noise:0.5".into();
        o.save_dir = Some(scratch(&format!("latency-{tag}")));
        let r = run_null(&o).unwrap();
        assert_eq!((r.original.latency_samples, r.port.latency_samples), (0, 37), "{tag}");
        assert!(r.latency_differs);
        assert!(r.to_text().contains("LATENCY DIFFERS"));
        // The port's aligned render is its raw output from sample 37 on: for the stub, gain times the input 37 samples later.
        let save = scratch(&format!("latency-{tag}"));
        let input: Audio = wav::read(&save.join("input.wav")).unwrap();
        let port_out = wav::read(&save.join("port.wav")).unwrap();
        let gain = port_out.channels[0][0] / input.channels[0][37];
        assert!(gain.is_finite() && gain != 0.0, "{tag}: gain {gain}");
        for n in [0usize, 1, 100, 5000, 23000] {
            let want = input.channels[0][n + 37] * gain;
            assert!((port_out.channels[0][n] - want).abs() <= 1e-6 * want.abs(), "{tag} sample {n}");
        }
        let original_out = wav::read(&save.join("original.wav")).unwrap();
        assert_eq!(original_out.channels[0][100], input.channels[0][100], "{tag}");
        // The last 37 aligned samples of the port are the zero padding the host fed it after the input.
        assert!(port_out.channels[0][23990..].iter().all(|v| *v == 0.0));
    }
}

fn assert_null_with_latency(r: &NullReport, latency: u32) {
    assert!(r.pass, "{}", r.to_text());
    assert_eq!((r.original.latency_samples, r.port.latency_samples), (latency, latency));
    assert!(!r.latency_differs);
    assert!(r.channels.iter().all(|c| c.max_abs_diff == 0.0));
}
