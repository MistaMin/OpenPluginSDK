//! `openplugin-port`: porting toolkit. See `docs/PORTING.md`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use openplugin_port::capture::{self, CaptureOptions};
use openplugin_port::null::{self, NullOptions};
use openplugin_port::scaffold::{self, NewOptions, SdkSource};
use openplugin_port::{ScanOptions, probe, render, scan, write_outputs};

const HELP: &str = "\
openplugin-port: move a plugin from another SDK to OpenPluginSDK

Usage:
  openplugin-port scan <plugin>... [--out DIR] [--timeout SECONDS] [--format FORMAT]

  <plugin>    a .clap, .vst3, .component or .aaxplugin path. The plugin is loaded
              from where it is; nothing is installed, copied or changed.
  --out DIR   where to write <name>.<format>.manifest.json and <name>.<format>.params.rs
              (default: the current folder)
  --timeout   seconds each plugin gets to load and answer (default 30)
  --format    clap, vst3, au or aax, when the extension does not say

  openplugin-port capture <plugin>... [--out DIR] [--format FORMAT] [--set PARAM=VALUE]...
                          [--load-component FILE] [--load-controller FILE]
                          [--max-presets N] [--timeout SECONDS]

  capture     load the plugin in a child process (nothing is installed) and save
              its state blobs exactly as a host stores them: CLAP state stream
              bytes, VST3 component and controller streams, the Audio Unit
              ClassInfo dictionary as a binary plist. One state for the default
              settings, one per factory preset (CLAP preset discovery, VST3
              program lists, Audio Unit factory presets), one after --load-component and
              --load-controller (VST3: files restored the way a host restores a session,
              to check what a port does with an old plugin's states), and one after --set
              (a parameter id or name and a plain value, repeatable). Writes
              <name>.<format>.capture.json and a <name>.<format>.blobs folder in
              --out (default: the current folder), with every parameter's plain
              value read back after each state was loaded.

  openplugin-port null <original> <port> [--format FMT] [--port-format FMT]
                       [--input SIGNAL|file.wav] [--automation SPEC] [--rate HZ]
                       [--block N] [--tolerance T] [--param-map map.json]
                       [--save DIR] [--json FILE] [--timeout SECONDS]
  openplugin-port param-map <original> <port> [--out map.json]

  null        render the original and the port (CLAP, VST3 or Audio Unit, each
              loaded by path in its own child process) through the same input and
              parameter automation, align them by their reported latencies and
              compare. Exit 0 when within the tolerance, 1 when not, 3 when a
              plugin could not be rendered (crash, hang, load failure).
  --input     sweep, noise, impulses, burst or silence, optionally ':seconds'
              (default sweep:2), or a 16/24/32-bit int or float WAV file
  --format    the original's format when the extension does not say; also the
              port's unless --port-format is given
  --automation  'frame:param=value' entries separated by commas; a time may end
              in s or ms; param is the original's id or name; '@file' reads a file
  --save DIR  write input, original, port and difference as WAV files
  param-map   write the original-to-port parameter id map the scanner manifests
              give (same name, else same id)

  openplugin-port new --from <name>.<format>.manifest.json [--out DIR] [--name CRATE]
                      [--sdk PATH | --sdk-version VERSION]

  new         write a plugin crate from a scan's manifest: the parameter table and
              PluginInfo, the identity the SDK can pin (the rest as TODO lines),
              a stub that reads the original's saved state, and a null test
              against the original that `cargo test` runs. Its audio passes the
              input through until the DSP is written.
  --out DIR   the crate's folder; must not exist or be empty (default: ./<name>)
  --name      the Cargo package name (default: made from the plugin's name)
  --sdk PATH  depend on an OpenPluginSDK checkout by path; --sdk-version on a
              crates.io release (default: this tool's version)

Each plugin is loaded in a child process, so a crash or a hang in the plugin
fails that one scan and nothing else. The exit code is 0 when every plugin scanned.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("__probe") => probe::child_main(&args[1..]),
        Some("__render") => render::child_main(&args[1..]),
        Some("__capture") => capture::child_main(&args[1..]),
        Some("capture") => run_capture(&args[1..]),
        Some("scan") => run_scan(&args[1..]),
        Some("null") => run_null(&args[1..]),
        Some("param-map") => run_param_map(&args[1..]),
        Some("new") => run_new(&args[1..]),
        Some("-h") | Some("--help") | Some("help") | None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("openplugin-port: unknown command {other}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn run_scan(args: &[String]) -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut out = PathBuf::from(".");
    let mut timeout = 30u64;
    let mut format: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => match it.next() {
                Some(v) => out = PathBuf::from(v),
                None => return usage("--out needs a folder"),
            },
            "--timeout" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => timeout = v,
                None => return usage("--timeout needs a number of seconds"),
            },
            "--format" => match it.next() {
                Some(v) => format = Some(v.clone()),
                None => return usage("--format needs a name"),
            },
            flag if flag.starts_with("--") => return usage(&format!("unknown option {flag}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    if paths.is_empty() {
        return usage("scan needs at least one plugin path");
    }
    let helper = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openplugin-port: cannot find myself to start the probe: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut failures = 0;
    for path in &paths {
        let mut opts = ScanOptions::new(helper.clone());
        opts.timeout = Duration::from_secs(timeout);
        opts.format = format.clone();
        match scan(path, &opts) {
            Ok(manifests) => match write_outputs(&manifests, &out) {
                Ok(files) => {
                    for m in &manifests {
                        println!(
                            "ok    {}  {} {:?} by {:?}: {} parameters, {} warnings",
                            path.display(),
                            m.source.format,
                            m.plugin.name,
                            m.plugin.vendor,
                            m.plugin.params.len(),
                            m.plugin.warnings.len()
                        );
                    }
                    for f in files {
                        println!("      wrote {}", f.display());
                    }
                }
                Err(e) => {
                    failures += 1;
                    println!("FAIL  {}: cannot write to {}: {e}", path.display(), out.display());
                }
            },
            Err(e) => {
                failures += 1;
                println!("FAIL  {}: {e}", path.display());
            }
        }
    }
    if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn run_capture(args: &[String]) -> ExitCode {
    let helper = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openplugin-port: cannot find myself to start the capture: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut o = CaptureOptions::new(helper, PathBuf::from("."));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let r: Result<(), String> = (|| {
            let mut value = |what: &str| it.next().cloned().ok_or_else(|| format!("{a} needs {what}"));
            match a.as_str() {
                "--out" => o.out_dir = PathBuf::from(value("a folder")?),
                "--format" => o.format = Some(value("a name")?),
                "--set" => o.sets.push(capture::parse_set(&value("param=value")?)?),
                "--load-component" => o.load_component = Some(PathBuf::from(value("a file")?)),
                "--load-controller" => o.load_controller = Some(PathBuf::from(value("a file")?)),
                "--max-presets" => o.max_presets = value("a number")?.parse().map_err(|_| "--max-presets needs a number")?,
                "--timeout" => o.timeout = Duration::from_secs(value("seconds")?.parse().map_err(|_| "--timeout needs a number of seconds")?),
                flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                path => paths.push(PathBuf::from(path)),
            }
            Ok(())
        })();
        if let Err(e) = r {
            return usage(&e);
        }
    }
    if paths.is_empty() {
        return usage("capture needs at least one plugin path");
    }
    let mut failures = 0;
    for path in &paths {
        match capture::run(path, &o) {
            Ok(list) => {
                for (c, file) in list {
                    let blobs: usize = c.states.iter().map(|s| s.blobs.len()).sum();
                    println!("ok    {}  {} {:?}: {} states, {} blobs, {} notes", path.display(), c.source.format, c.plugin.name, c.states.len(), blobs, c.notes.len());
                    println!("      wrote {}", file.display());
                    for n in &c.notes {
                        println!("      note: {n}");
                    }
                }
            }
            Err(e) => {
                failures += 1;
                println!("FAIL  {}: {e}", path.display());
            }
        }
    }
    if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn run_null(args: &[String]) -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let helper = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openplugin-port: cannot find myself to start the renderer: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut o = NullOptions::new(helper, PathBuf::new(), PathBuf::new());
    let mut json: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let r: Result<(), String> = (|| {
            let mut value = |what: &str| it.next().cloned().ok_or_else(|| format!("{a} needs {what}"));
            match a.as_str() {
                "--format" => o.original_format = Some(value("a format")?),
                "--port-format" => o.port_format = Some(value("a format")?),
                "--input" => o.input = value("a signal or a WAV file")?,
                "--automation" => o.automation = Some(value("a spec")?),
                "--rate" => o.rate = Some(value("a rate")?.parse().map_err(|_| "--rate needs a number")?),
                "--block" => o.block = value("a number")?.parse().map_err(|_| "--block needs a number")?,
                "--tolerance" => o.tolerance = value("a number")?.parse().map_err(|_| "--tolerance needs a number")?,
                "--param-map" => o.param_map = Some(PathBuf::from(value("a file")?)),
                "--save" => o.save_dir = Some(PathBuf::from(value("a folder")?)),
                "--json" => json = Some(PathBuf::from(value("a file")?)),
                "--timeout" => o.timeout = Duration::from_secs(value("seconds")?.parse().map_err(|_| "--timeout needs a number of seconds")?),
                flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                path => paths.push(PathBuf::from(path)),
            }
            Ok(())
        })();
        if let Err(e) = r {
            return usage(&e);
        }
    }
    if paths.len() != 2 {
        return usage("null needs the original plugin and the port");
    }
    o.original = paths[0].clone();
    o.port = paths[1].clone();
    if o.port_format.is_none() {
        o.port_format = o.original_format.clone();
    }
    match null::run_null(&o) {
        Ok(report) => {
            print!("{}", report.to_text());
            if let Some(j) = json {
                if let Err(e) = std::fs::write(&j, serde_json::to_string_pretty(&report).unwrap_or_default() + "\n") {
                    eprintln!("openplugin-port: cannot write {}: {e}", j.display());
                    return ExitCode::from(3);
                }
            }
            if report.pass { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Err(e) => {
            println!("ERROR {e}");
            ExitCode::from(3)
        }
    }
}

fn run_param_map(args: &[String]) -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => match it.next() {
                Some(v) => out = Some(PathBuf::from(v)),
                None => return usage("--out needs a file"),
            },
            flag if flag.starts_with("--") => return usage(&format!("unknown option {flag}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    if paths.len() != 2 {
        return usage("param-map needs the original plugin and the port");
    }
    let helper = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openplugin-port: cannot find myself to start the probe: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut manifests = Vec::new();
    for p in &paths {
        match scan(p, &ScanOptions::new(helper.clone())) {
            Ok(mut m) => manifests.push(m.remove(0)),
            Err(e) => {
                println!("FAIL  {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let (map, unmatched) = null::generate_param_map(&manifests[0], &manifests[1], &[]);
    let text = serde_json::to_string_pretty(&map).unwrap_or_default() + "\n";
    match out {
        Some(f) => {
            if let Err(e) = std::fs::write(&f, &text) {
                eprintln!("openplugin-port: cannot write {}: {e}", f.display());
                return ExitCode::FAILURE;
            }
            println!("wrote {}", f.display());
        }
        None => print!("{text}"),
    }
    for u in &unmatched {
        eprintln!("no port parameter for the original's {u}: add it by hand");
    }
    if unmatched.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn run_new(args: &[String]) -> ExitCode {
    let mut from: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut sdk = SdkSource::Version(env!("CARGO_PKG_VERSION").to_string());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let r: Result<(), String> = (|| {
            let mut value = |what: &str| it.next().cloned().ok_or_else(|| format!("{a} needs {what}"));
            match a.as_str() {
                "--from" => from = Some(PathBuf::from(value("a manifest")?)),
                "--out" => out = Some(PathBuf::from(value("a folder")?)),
                "--name" => name = Some(value("a crate name")?),
                "--sdk" => sdk = SdkSource::Path(std::path::absolute(value("a folder")?).map_err(|e| e.to_string())?),
                "--sdk-version" => sdk = SdkSource::Version(value("a version")?),
                other => return Err(format!("unknown option {other}")),
            }
            Ok(())
        })();
        if let Err(e) = r {
            return usage(&e);
        }
    }
    let Some(from) = from else {
        return usage("new needs --from <manifest>");
    };
    let tool = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openplugin-port: cannot find myself to name in the null test: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = scaffold::read_manifest(&from).and_then(|m| {
        let opts = NewOptions { sdk, crate_name: name, tool };
        let files = scaffold::generate(&m, &opts)?;
        let dir = out.unwrap_or_else(|| PathBuf::from(opts.crate_name.clone().unwrap_or_else(|| scaffold::crate_name(&m))));
        scaffold::write(&files, &dir).map(|w| (m, dir, w))
    });
    match result {
        Ok((m, dir, written)) => {
            println!("ok    {} {:?}: {} parameters, crate in {}", m.source.format, m.plugin.name, m.plugin.params.len(), dir.display());
            for f in written {
                println!("      wrote {}", f.display());
            }
            println!("      next: cargo test --manifest-path {:?}, then the TODO(porting lines", dir.join("Cargo.toml"));
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("FAIL  {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage(why: &str) -> ExitCode {
    eprintln!("openplugin-port: {why}\n\n{HELP}");
    ExitCode::from(2)
}
