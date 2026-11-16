<#
.SYNOPSIS
Runs a command in the logged-on user's interactive desktop session and
returns its exit code, stdout and stderr.

.DESCRIPTION
Programs started over SSH run in session 0, where windows cannot be shown.
This script registers a one-off scheduled task with an interactive logon for
the user who is logged on at the console (no password: an interactive task
for a logged-on user needs none), starts it, waits until the command has
finished or the timeout has passed, and always deletes the task again.

The command's stdout and stderr go to files in a run folder; when it ends,
this script writes them to its own stdout and stderr and exits with the
command's exit code. Exit 124 means the timeout passed (the command and every
process it started are killed); exit 125 means the runner itself failed.

The task runs with the user's normal logon environment, not the SSH
session's: variables set in the calling shell are not seen. Pass what the
command needs inside the command itself.

.PARAMETER Command
PowerShell code to run (Windows PowerShell 5.1). The exit code is the code's
`exit N`, else the last native program's exit code, else 1 when the last
statement failed, else 0.

.PARAMETER Script
A file to run instead: a .ps1 (run with powershell -File), a .cmd or .bat,
or an .exe. Arguments go in -ArgumentList.

.PARAMETER ArgumentList
Arguments for -Script.

.PARAMETER TimeoutSec
How long to wait before killing the command (default 300).

.PARAMETER WorkingDirectory
Where the command starts (default: the current directory).

.PARAMETER RunDir
The run folder for stdout.txt, stderr.txt and exitcode.txt (default: a new
folder under $env:TEMP\openplugin-desktop-runs). It is kept afterwards.

.PARAMETER User
The account to run as (default: the user running this script).

.EXAMPLE
powershell -NoProfile -File scripts\windows\run_in_desktop.ps1 -TimeoutSec 60 -Command "cargo test -p example-editor --test win32"

.EXAMPLE
powershell -NoProfile -File scripts\windows\run_in_desktop.ps1 -Script C:\path\gui_check.ps1 -ArgumentList one,two
#>
[CmdletBinding(DefaultParameterSetName = 'Command')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Command', Position = 0)]
    [string] $Command,
    [Parameter(Mandatory = $true, ParameterSetName = 'Script')]
    [string] $Script,
    [Parameter(ParameterSetName = 'Script')]
    [string[]] $ArgumentList = @(),
    [int] $TimeoutSec = 300,
    [string] $WorkingDirectory = (Get-Location).ProviderPath,
    [string] $RunDir,
    [string] $User = [Security.Principal.WindowsIdentity]::GetCurrent().Name
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$TaskPath = '\OpenPluginSDK\'

function Say([string] $text) { [Console]::Error.WriteLine("run_in_desktop: $text") }

# cmd.exe quoting for one argument.
function Quote-Cmd([string] $s) {
    if ($s -eq '') { return '""' }
    if ($s -notmatch '[\s"&|<>^()%!,;=]') { return $s }
    return '"' + ($s -replace '"', '""') + '"'
}

# Tasks a killed runner left behind: each one's description holds the time
# after which it can be removed. Running ones and other agents' live ones stay.
function Remove-StaleTasks {
    $now = [DateTime]::UtcNow
    foreach ($t in @(Get-ScheduledTask -TaskPath $TaskPath -ErrorAction SilentlyContinue)) {
        if ($t.TaskName -notlike 'run-*' -or $t.State -eq 'Running') { continue }
        if ($t.Description -match 'remove-after=(\S+)') {
            $after = [DateTime]::Parse($Matches[1], $null, [Globalization.DateTimeStyles]::RoundtripKind)
            if ($now -gt $after) {
                Unregister-ScheduledTask -TaskName $t.TaskName -TaskPath $TaskPath -Confirm:$false -ErrorAction SilentlyContinue
            }
        }
    }
}

# Kills the wrapper and everything it started, and waits until the wrapper
# has exited.
function Stop-Tree([string] $pidFile) {
    # taskkill reports already-gone processes on stderr; that is not an error.
    $ErrorActionPreference = 'Continue'
    if (Test-Path -LiteralPath $pidFile) {
        $wrapperPid = (Get-Content -LiteralPath $pidFile -Raw).Trim()
        if ($wrapperPid) {
            & taskkill.exe /T /F /PID $wrapperPid 2>&1 | Out-Null
            Wait-Process -Id $wrapperPid -Timeout 10 -ErrorAction SilentlyContinue
        }
    }
}

# Reads a file another process may still have open (a killed command's
# output file is released a moment after the kill).
function Read-Shared([string] $file) {
    if (-not (Test-Path -LiteralPath $file)) { return '' }
    $stream = New-Object IO.FileStream($file, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]'ReadWrite, Delete')
    try { return (New-Object IO.StreamReader($stream)).ReadToEnd() } finally { $stream.Dispose() }
}

$name = 'run-{0}-{1}' -f (Get-Date -Format 'yyyyMMdd-HHmmss'), ([Guid]::NewGuid().ToString('N').Substring(0, 8))
if (-not $RunDir) { $RunDir = Join-Path $env:TEMP "openplugin-desktop-runs\$name" }
$code = 125
$registered = $false
try {
    Remove-StaleTasks
    New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
    $RunDir = (Resolve-Path -LiteralPath $RunDir).ProviderPath
    $WorkingDirectory = (Resolve-Path -LiteralPath $WorkingDirectory).ProviderPath
    $out = Join-Path $RunDir 'stdout.txt'
    $err = Join-Path $RunDir 'stderr.txt'
    $done = Join-Path $RunDir 'exitcode.txt'
    $pidFile = Join-Path $RunDir 'pid.txt'
    foreach ($f in $out, $err, $done, $pidFile) { Remove-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue }

    $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if ($PSCmdlet.ParameterSetName -eq 'Command') {
        $payload = Join-Path $RunDir 'payload.ps1'
        $body = $Command + "`r`n" + 'if (-not $?) { exit 1 }; if ($LASTEXITCODE) { exit $LASTEXITCODE }; exit 0' + "`r`n"
        [IO.File]::WriteAllText($payload, $body, (New-Object Text.UTF8Encoding $true))
        $line = "$(Quote-Cmd $ps) -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $(Quote-Cmd $payload)"
    } else {
        $target = (Resolve-Path -LiteralPath $Script).ProviderPath
        $quotedArgs = ($ArgumentList | ForEach-Object { Quote-Cmd $_ }) -join ' '
        switch ([IO.Path]::GetExtension($target).ToLowerInvariant()) {
            '.ps1' { $line = "$(Quote-Cmd $ps) -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $(Quote-Cmd $target) $quotedArgs" }
            { $_ -in '.cmd', '.bat' } { $line = "call $(Quote-Cmd $target) $quotedArgs" }
            default { $line = "$(Quote-Cmd $target) $quotedArgs" }
        }
    }

    # payload.cmd does the redirection (cmd.exe passes bytes through as they
    # are); wrapper.ps1 records its own PID so a timeout can kill the tree,
    # then publishes the exit code with a rename so a reader never sees a
    # half-written file.
    $payloadCmd = Join-Path $RunDir 'payload.cmd'
    $cmdText = "@echo off`r`ncd /d $(Quote-Cmd $WorkingDirectory)`r`n$line < NUL > $(Quote-Cmd $out) 2> $(Quote-Cmd $err)`r`nexit /b %ERRORLEVEL%`r`n"
    [IO.File]::WriteAllText($payloadCmd, $cmdText, (New-Object Text.ASCIIEncoding))
    $wrapper = Join-Path $RunDir 'wrapper.ps1'
    $q = { param($s) "'" + ($s -replace "'", "''") + "'" }
    $wrapperText = @"
`$PID | Set-Content -LiteralPath $(& $q $pidFile) -Encoding ASCII
& `$env:ComSpec /d /c $(& $q $payloadCmd)
`$c = `$LASTEXITCODE
Set-Content -LiteralPath $(& $q "$done.tmp") -Value `$c -Encoding ASCII
Move-Item -LiteralPath $(& $q "$done.tmp") -Destination $(& $q $done) -Force
exit `$c
"@
    [IO.File]::WriteAllText($wrapper, $wrapperText, (New-Object Text.UTF8Encoding $true))

    $removeAfter = [DateTime]::UtcNow.AddSeconds($TimeoutSec + 600).ToString('o')
    $action = New-ScheduledTaskAction -Execute $ps -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$wrapper`"" -WorkingDirectory $WorkingDirectory
    $principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit (New-TimeSpan -Seconds ($TimeoutSec + 120)) -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskName $name -TaskPath $TaskPath -Action $action -Principal $principal -Settings $settings `
        -Description "OpenPluginSDK run_in_desktop.ps1; remove-after=$removeAfter" | Out-Null
    $registered = $true
    Start-ScheduledTask -TaskName $name -TaskPath $TaskPath

    $clock = [Diagnostics.Stopwatch]::StartNew()
    $startLimit = [Math]::Min(30, $TimeoutSec)
    $timedOut = $false
    while (-not (Test-Path -LiteralPath $done)) {
        if ($clock.Elapsed.TotalSeconds -ge $TimeoutSec) { $timedOut = $true; break }
        # The task never started: usually nobody is logged on at the desktop.
        if (-not (Test-Path -LiteralPath $pidFile) -and $clock.Elapsed.TotalSeconds -ge $startLimit) {
            $info = Get-ScheduledTaskInfo -TaskName $name -TaskPath $TaskPath
            throw "the task did not start within $startLimit s (last result $($info.LastTaskResult)); is $User logged on at the desktop? (query user)"
        }
        Start-Sleep -Milliseconds 200
    }

    if ($timedOut) {
        Stop-Tree $pidFile
        Stop-ScheduledTask -TaskName $name -TaskPath $TaskPath -ErrorAction SilentlyContinue
        $code = 124
    } else {
        $code = [int]((Get-Content -LiteralPath $done -Raw).Trim())
    }
    $s = Read-Shared $out; if ($s) { [Console]::Out.Write($s) }
    $s = Read-Shared $err; if ($s) { [Console]::Error.Write($s) }
    if ($timedOut) { Say "timed out after $TimeoutSec s; killed. Run folder: $RunDir" }
    else { Say "exit $code. Run folder: $RunDir" }
} catch {
    Say "error: $($_.Exception.Message)"
    if ($pidFile) { Stop-Tree $pidFile }
    $code = 125
} finally {
    if ($registered) {
        Unregister-ScheduledTask -TaskName $name -TaskPath $TaskPath -Confirm:$false -ErrorAction SilentlyContinue
    }
}
exit $code
