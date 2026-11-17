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
    // A process the plugin starts can inherit stdout and hold it open for good, so nothing waits for EOF.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n @ 1..) = stdout.read(&mut chunk) {
            if tx.send(chunk[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s; the plugin was killed", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(format!("waiting for the probe: {e}")),
        }
    };
    let mut bytes = Vec::new();
    while !has_result(&bytes) {
        match rx.recv_timeout(timeout.saturating_sub(start.elapsed())) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }
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

/// True once `bytes` holds a whole result line.
fn has_result(bytes: &[u8]) -> bool {
    bytes.split(|&b| b == b'\n').rev().skip(1).any(|line| line.starts_with(MARK.as_bytes()))
}

#[cfg(unix)]
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

/// Windows has no signals: a crash ends the process with its exception code
/// (an NTSTATUS whose top two bits are set) as the exit code.
#[cfg(windows)]
fn describe_exit(status: &std::process::ExitStatus) -> String {
    let code = status.code().unwrap_or(-1) as u32;
    if code & 0xC000_0000 != 0xC000_0000 {
        return format!("the probe process exited with {} without a result", code as i32);
    }
    let name = match code {
        0xC000_0005 => "access violation",
        0xC000_001D => "illegal instruction",
        0xC000_0094 => "integer divide by zero",
        0xC000_00FD => "stack overflow",
        0xC000_0409 => "fast fail, as from abort",
        _ => "an exception",
    };
    format!("the plugin crashed the probe process ({name}, 0x{code:08X})")
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Duration) -> (Result<u32, String>, Duration) {
        let start = Instant::now();
        let result = run_helper::<u32>(Path::new("/bin/sh"), &["-c".into(), script.into()], timeout, &[]);
        (result, start.elapsed())
    }

    #[test]
    fn a_grandchild_holding_stdout_does_not_outlast_the_timeout() {
        let (result, took) = sh("sleep 5 & exec sleep 5", Duration::from_millis(300));
        assert!(result.unwrap_err().contains("timed out"));
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    #[test]
    fn a_grandchild_holding_stdout_does_not_hide_the_result() {
        let (result, took) = sh(&format!("sleep 5 & echo '{MARK}{{\"Ok\":7}}'"), Duration::from_secs(4));
        assert_eq!(result, Ok(7));
        assert!(took < Duration::from_secs(4), "{took:?}");
    }
}
