# Windows helper scripts

Tools for checking Windows work without anyone at the PC, typically from an
SSH session. Windows PowerShell 5.1 runs them; the machine's execution policy
blocks `.ps1` files by default, so call them as
`powershell -NoProfile -ExecutionPolicy Bypass -File <script> ...`.

## run_in_desktop.ps1: run a command where windows can be seen

A program started over SSH runs in session 0, which has no visible desktop:
its windows never appear and screen captures are blank. This script runs a
command in the desktop session of the user who is logged on at the PC, through
a one-off scheduled task (interactive logon, highest privileges, no password),
waits for it, and deletes the task again, also on a timeout or an error.

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run_in_desktop.ps1 `
    [-TimeoutSec 300] [-WorkingDirectory <dir>] [-RunDir <dir>] -Command "<PowerShell code>"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run_in_desktop.ps1 `
    [-TimeoutSec 300] -Script <file.ps1|.cmd|.bat|.exe> [-ArgumentList a,b,c]
```

- `-Command` is Windows PowerShell code. Its exit code is its `exit N`, else
  the last native program's exit code, else 1 when the last statement
  failed, else 0.
- `-Script` runs a file with arguments.
- The command starts in `-WorkingDirectory` (default: the current directory).
- The command's stdout and stderr come back on the runner's stdout and
  stderr, after it has finished. The runner exits with the command's exit
  code; **124** means the timeout passed (the command and every process it
  started were killed), **125** means the runner failed (for example nobody
  is logged on; see `query user`).
- The last stderr line is `run_in_desktop: exit N. Run folder: <dir>`. The run
  folder (default `%TEMP%\openplugin-desktop-runs\run-<time>-<id>`) keeps
  `stdout.txt`, `stderr.txt`, `exitcode.txt` and the generated scripts.
- The task runs with the user's own logon environment, not the SSH session's:
  variables set in the calling shell are not passed on. Set what the command
  needs inside `-Command`.
- A program the command starts in the background (`Start-Process`) keeps
  running after the command returns, so a window can be opened, then
  screenshotted, then closed with a second call.
- Several runs at once are fine; each has its own task and run folder. Tasks
  live under `\OpenPluginSDK\` in Task Scheduler; one a killed runner left
  behind is removed by a later run once its time is past.

Examples:

```
# A test that opens windows, from the worktree:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run_in_desktop.ps1 -TimeoutSec 600 `
    -Command "cargo test -p example-editor --test win32 -- --nocapture"

# Open REAPER with a project and leave it running:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run_in_desktop.ps1 `
    -Command "Start-Process 'C:\Program Files\REAPER (x64)\reaper.exe' -ArgumentList 'C:\path\test.rpp'"
```

From the Mac: `ssh pc "cd /d C:\Users\marco\Dependencies\OpenPluginSDK-<area> && powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run_in_desktop.ps1 -Command ""cargo test ..."""`,
or write the call into a PowerShell script for the agents' `pcrun.sh` helper.

## screenshot.ps1: a PNG of the desktop or of one window

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\screenshot.ps1 -Path <out.png> `
    [-Title <text> [-Exact] [-WaitSec 10] [-FromScreen] [-Foreground]]
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\screenshot.ps1 -List
```

- Without `-Title`: the whole desktop, every monitor.
- With `-Title`: the first visible top-level window whose title contains the
  text (`-Exact`: equals it, ignoring case). It waits up to `-WaitSec` for the
  window to appear, so it can be called right after starting a program. The
  capture is the window's own pixels (PrintWindow), even when something
  covers it; `-FromScreen` copies its area from the screen instead.
- `-List` prints the visible windows (handle, process, size, title).
- Called over SSH, it runs itself through `run_in_desktop.ps1`. Prints the
  PNG's path and size; exit 1 when no window matched (the visible windows are
  listed on stderr).

## selftest.ps1

Proves both scripts on this PC: exit code, stdout and stderr come back, a
timeout kills the command (exit 124), and a WinForms window opened through the
runner is on the desktop and its screenshot shows the window. Pictures go to
`%TEMP%\openplugin-desktop-selftest`.

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\selftest.ps1
```

## pluginval

pluginval 1.0.4 (the latest release that is not a prerelease, as in the
macOS CI job) is installed at `C:\Users\marco\Tools\pluginval\pluginval.exe`.
`pluginval_Windows.zip` has SHA-256
`c08e61ce3b96db41636f8ec7e76f4c7e2c13ebdac7fa1b5a1f52b4f32ec715ab`; the release
page publishes no checksum, so this one was computed from downloads on two
machines, which matched. The zip is kept next to the program.

Headless (no editor), over SSH is fine; it exits non-zero when any test fails:

```
C:\Users\marco\Tools\pluginval\pluginval.exe --strictness-level 10 --skip-gui-tests `
    --output-dir "$env:TEMP\pluginval-vst3" --validate "<path>\Example Gain.vst3"
```

With the editor tests (they open the plug-in's window), run it in the
desktop session and leave out `--skip-gui-tests`:

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\run_in_desktop.ps1 -TimeoutSec 900 `
    -Command "& 'C:\Users\marco\Tools\pluginval\pluginval.exe' --strictness-level 10 --output-dir `$env:TEMP\pluginval-vst3 --validate 'C:\path\Example Gain.vst3'"
```

`--validate <path>` names the plug-in (for a VST3, the `.vst3` bundle folder);
pluginval 1.0.4 does not load CLAP. pluginval.exe is a windowed (GUI
subsystem) program: PowerShell waits for it and sets `$LASTEXITCODE` only
when its output is piped or redirected (`& pluginval.exe ... | Out-Host`, or
`*> log.txt`); the runner above always redirects it. `pluginval.exe --help`
lists the options.
