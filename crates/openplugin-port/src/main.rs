//! `openplugin-port`: porting toolkit. See `docs/PORTING.md`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use openplugin_port::{ScanOptions, probe, scan, write_outputs};

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

Each plugin is loaded in a child process, so a crash or a hang in the plugin
fails that one scan and nothing else. The exit code is 0 when every plugin scanned.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("__probe") => probe::child_main(&args[1..]),
        Some("scan") => run_scan(&args[1..]),
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

fn usage(why: &str) -> ExitCode {
    eprintln!("openplugin-port: {why}\n\n{HELP}");
    ExitCode::from(2)
}
