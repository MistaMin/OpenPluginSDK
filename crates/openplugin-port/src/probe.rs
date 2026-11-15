//! Process isolation. A plugin is loaded only in a child process: the scanner
//! re-executes itself with the hidden subcommand `__probe <format> <path>`;
//! the child loads the plugin, prints one result line and exits without
//! running the plugin's teardown. The parent waits with a timeout, so a plugin
//! that crashes, hangs or prints noise cannot harm the scan.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::manifest::ProbeResult;
use crate::{aax, au, bundle, clap, sys, vst3};

/// A line of the child's output starts with this; everything else is the plugin's noise.
const MARK: &str = "OPENPLUGIN_PORT_RESULT ";

/// Test hook: `crash` makes the child abort and `hang` makes it sleep, before it loads anything.
pub const FAULT_ENV: &str = "OPENPLUGIN_PORT_FAULT";

/// Run the probe for `format` on `path` in a child process.
pub fn run_child(helper: &Path, format: &str, path: &Path, timeout: Duration, env: &[(String, String)]) -> Result<ProbeResult, String> {
    run_helper(helper, &["__probe".into(), format.into(), path.as_os_str().to_owned()], timeout, env)
}

/// Run the helper executable with `args` and read the one result line it prints.
/// The helper is killed at `timeout`; a crash becomes an error message.
pub fn run_helper<T: serde::de::DeserializeOwned>(helper: &Path, args: &[std::ffi::OsString], timeout: Duration, env: &[(String, String)]) -> Result<T, String> {
    let mut child = Command::new(helper)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the probe process {}: {e}", helper.display()))?;
    let mut stdout = child.stdout.take().ok_or("no probe stdout")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("timed out after {} s; the plugin was killed", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(format!("waiting for the probe: {e}")),
        }
    };
    let bytes = reader.join().unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let line = text.lines().rev().find_map(|l| l.strip_prefix(MARK));
    match line {
        Some(json) => match serde_json::from_str::<Result<T, String>>(json) {
            Ok(r) => r,
            Err(e) => Err(format!("unreadable probe result: {e}")),
        },
        None => Err(describe_exit(&status)),
    }
}

fn describe_exit(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(sig) = status.signal() {
        let name = match sig {
            4 => "SIGILL",
            6 => "SIGABRT",
            10 => "SIGBUS",
            11 => "SIGSEGV",
            _ => "a signal",
        };
        format!("the plugin crashed the probe process ({name}, signal {sig})")
    } else {
        format!("the probe process exited with {} without a result", status.code().unwrap_or(-1))
    }
}

/// The child's entry point: `__probe <format> <path>`. Never returns.
pub fn child_main(args: &[String]) -> ! {
    let result = child_probe(args);
    emit_result(&result)
}

/// Print the one marked result line and leave without running any teardown. Never returns.
pub fn emit_result<T: serde::Serialize>(result: &Result<T, String>) -> ! {
    let json = serde_json::to_string(result).unwrap_or_else(|e| format!("{{\"Err\":\"cannot encode the result: {e}\"}}"));
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "{MARK}{json}");
    let _ = out.flush();
    // The plugin's own teardown is the likeliest thing to crash; skip it.
    sys::exit_now(0)
}

fn child_probe(args: &[String]) -> Result<ProbeResult, String> {
    let (Some(format), Some(path)) = (args.first(), args.get(1)) else {
        return Err("usage: __probe <format> <path>".into());
    };
    match std::env::var(FAULT_ENV).as_deref() {
        Ok("crash") => std::process::abort(),
        Ok("hang") => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        _ => {}
    }
    let path = PathBuf::from(path);
    match format.as_str() {
        "clap" => {
            let exe = bundle::executable(&path)?;
            clap::probe(&path, &exe)
        }
        "vst3" => vst3::probe(&path),
        "au" => au::probe(&path),
        "aax" => aax::probe(&path),
        other => Err(format!("unknown format {other}")),
    }
}
