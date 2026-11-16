<#
.SYNOPSIS
Checks run_in_desktop.ps1 and screenshot.ps1 on this machine.

.DESCRIPTION
Runs from SSH (session 0). Checks that the runner returns stdout, stderr and
the exit code, that a timeout kills the command and exits 124, and that a
WinForms window opened through the runner is visible on the desktop: it
screenshots the window by title and checks the PNG's size and that it is not
blank. Leaves the PNG at -OutDir\window.png (and desktop.png) to look at.
Exit 0 when everything passed.
#>
param([string] $OutDir = (Join-Path $env:TEMP 'openplugin-desktop-selftest'))

$ErrorActionPreference = 'Stop'
$runner = Join-Path $PSScriptRoot 'run_in_desktop.ps1'
$shot = Join-Path $PSScriptRoot 'screenshot.ps1'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$failed = 0
# The runner as its own process, as a caller over SSH runs it, so its stdout
# and stderr are real streams.
function Run-Desktop {
    $ErrorActionPreference = 'Continue'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $runner @args
}
function Check([string] $name, $ok, [string] $detail) {
    if ([bool]$ok) { "PASS $name" } else { "FAIL $name : $detail"; $script:failed++ }
}

# 1. Output, error output and exit code come back.
$o = Run-Desktop -TimeoutSec 60 -Command "'hello from session ' + (Get-Process -Id `$PID).SessionId; [Console]::Error.WriteLine('to stderr'); exit 3" 2> (Join-Path $OutDir 'err1.txt')
$c = $LASTEXITCODE
$e = [string](Get-Content -Raw (Join-Path $OutDir 'err1.txt'))
Check 'exit code' ($c -eq 3) "got $c"
Check 'stdout, not session 0' (("$o" -match 'hello from session [1-9]')) "got '$o'"
Check 'stderr' ($e -match 'to stderr') "got '$e'"

# 2. A timeout kills the command and its children.
$pidFile = Join-Path $OutDir 'sleeper.pid'
Remove-Item -LiteralPath $pidFile -ErrorAction SilentlyContinue
$clock = [Diagnostics.Stopwatch]::StartNew()
Run-Desktop -TimeoutSec 8 -Command "`$PID | Set-Content -LiteralPath '$pidFile'; Start-Sleep -Seconds 120" 2> (Join-Path $OutDir 'err2.txt') | Out-Null
$c = $LASTEXITCODE
Check 'timeout exit 124' ($c -eq 124) "got $c; $(Get-Content -Raw (Join-Path $OutDir 'err2.txt'))"
Check 'timeout is prompt' ($clock.Elapsed.TotalSeconds -lt 30) "took $($clock.Elapsed.TotalSeconds) s"
$sleeper = if (Test-Path -LiteralPath $pidFile) { [int](Get-Content -Raw -LiteralPath $pidFile).Trim() } else { 0 }
Check 'timeout: command had started' ($sleeper -ne 0) 'no pid written'
Check 'timeout killed the command' (-not (Get-Process -Id $sleeper -ErrorAction SilentlyContinue)) "pid $sleeper still running"

# 3. A window opened through the runner shows on the desktop.
$title = "OpenPluginSDK desktop check $([Guid]::NewGuid().ToString('N').Substring(0, 6))"
$form = Join-Path $OutDir 'window.ps1'
@"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
`$f = New-Object Windows.Forms.Form
`$f.Text = '$title'
`$f.Width = 520; `$f.Height = 220; `$f.StartPosition = 'CenterScreen'; `$f.TopMost = `$true
`$f.BackColor = [Drawing.Color]::FromArgb(30, 90, 160)
`$l = New-Object Windows.Forms.Label
`$l.Text = 'Shown by run_in_desktop.ps1'
`$l.ForeColor = [Drawing.Color]::White
`$l.Font = New-Object Drawing.Font('Segoe UI', 20)
`$l.AutoSize = `$true; `$l.Left = 20; `$l.Top = 50
`$f.Controls.Add(`$l)
`$t = New-Object Windows.Forms.Timer
`$t.Interval = 30000; `$t.Add_Tick({ `$f.Close() }); `$t.Start()
[void]`$f.ShowDialog()
"@ | Set-Content -LiteralPath $form -Encoding UTF8
Run-Desktop -TimeoutSec 60 -Command "Start-Process powershell.exe -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ' + [char]34 + '$form' + [char]34)" 2>$null | Out-Null
Check 'window launched' ($LASTEXITCODE -eq 0) "runner exit $LASTEXITCODE"
$png = Join-Path $OutDir 'window.png'
Remove-Item -LiteralPath $png -ErrorAction SilentlyContinue
& $shot -Title $title -WaitSec 20 -Path $png
Check 'window screenshot' (($LASTEXITCODE -eq 0) -and (Test-Path -LiteralPath $png)) "screenshot exit $LASTEXITCODE"
if (Test-Path -LiteralPath $png) {
    Add-Type -AssemblyName System.Drawing
    $b = [Drawing.Bitmap]::FromFile($png)
    # The middle-left of the client area is the form's blue background.
    $px = $b.GetPixel([int]($b.Width * 0.03), [int]($b.Height * 0.9))
    $size = "$($b.Width)x$($b.Height)"
    $b.Dispose()
    Check 'window size' ($size -ne '0x0') $size
    Check 'window pixels are the form' (($px.B -gt 120) -and ($px.R -lt 80)) "pixel $px"
}
& $shot -Path (Join-Path $OutDir 'desktop.png') | Out-Null
Check 'desktop screenshot' ($LASTEXITCODE -eq 0) "exit $LASTEXITCODE"
# Close the check window now rather than waiting for its timer.
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" | Where-Object { $_.CommandLine -like "*$form*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

if ($failed) { "$failed check(s) failed"; exit 1 }
"all checks passed; pictures in $OutDir"
exit 0
