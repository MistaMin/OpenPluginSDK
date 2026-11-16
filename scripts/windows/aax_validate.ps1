# SPDX-License-Identifier: MIT
# Run Avid's AAX Validator (DigiShell "aaxval" dish) against the example AAX
# plug-in on Windows, locally. The validator and the AAX SDK are not
# redistributable, so this cannot run in public CI. The Windows counterpart of
# scripts/aax_validate.sh; see docs/AAX_VALIDATOR.md.
#
# Usage (PowerShell, from anywhere):
#   $env:AAX_VALIDATOR_DIR = 'C:\path\to\DigiShell'      # holds dsh.exe
#   $env:AAX_SDK = 'C:\path\to\aax-sdk'                  # holds Interfaces\AAX.h
#   scripts\windows\aax_validate.ps1 [-Bundle C:\path\to\X.aaxplugin] [-Out DIR]
#
# Without -Bundle the script builds examples/editor as a static library and the
# AAX shell (AAX only, with page tables) under Out. Nothing is installed or
# signed; the plug-in is validated in place, by path.
#
# Environment:
#   AAX_VALIDATOR_DIR      folder holding dsh.exe (required)
#   AAX_SDK                AAX SDK checkout (required unless -Bundle is given)
#   AAX_VALIDATOR_OUT      build and log folder (default: build-aax-validator in the repo)
#   AAX_VALIDATOR_TESTS    space separated test ids to run (default: every test)
#   OP_EDITOR              ON (default) or OFF, whether the shell hosts the editor
#   AAX_VALIDATOR_DSP      1 to count test.cycle_counts (needs HDX); default skips it
#   AAX_VALIDATOR_TIMEOUT  seconds for the whole dsh run (default 3600)
#
# Exit code: 0 no test failed, 1 a test failed or there were no results, 2 the
# script could not start (validator or SDK missing, build failed).
param(
    [string]$Bundle = '',
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Out) { $Out = if ($env:AAX_VALIDATOR_OUT) { $env:AAX_VALIDATOR_OUT } else { Join-Path $root 'build-aax-validator' } }

$validator = $env:AAX_VALIDATOR_DIR
$dsh = if ($validator) { Join-Path $validator 'dsh.exe' } else { '' }
if (-not $validator -or -not (Test-Path -LiteralPath $dsh -PathType Leaf)) {
    [Console]::Error.WriteLine("aax_validate: AAX Validator not found.`nSet AAX_VALIDATOR_DIR to the unpacked DigiShell folder, the one that contains dsh.exe.`nIt comes from the Avid developer site and is not redistributable.")
    exit 2
}
if ($validator -match '\s') {
    [Console]::Error.WriteLine("aax_validate: AAX_VALIDATOR_DIR contains a space: $validator`ndsh starts each test in a child process and passes it the validator's path unquoted. Use a path without spaces.")
    exit 2
}

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
$log = Join-Path $Out 'aax_validate.log'
Set-Content -LiteralPath $log -Value '' -Encoding utf8

# Runs a native command with its output appended to the log; returns its exit code.
function Invoke-Logged([string]$exe, [string[]]$arguments) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $exe @arguments 2>&1 | ForEach-Object { "$_" } | Add-Content -LiteralPath $log -Encoding utf8 }
    finally { $ErrorActionPreference = $previous }
    return $LASTEXITCODE
}

if (-not $Bundle) {
    $sdk = $env:AAX_SDK
    if (-not $sdk -or -not (Test-Path -LiteralPath (Join-Path $sdk 'Interfaces\AAX.h'))) {
        [Console]::Error.WriteLine('aax_validate: set AAX_SDK to the AAX SDK checkout (needs Interfaces\AAX.h), or pass -Bundle')
        exit 2
    }
    "building example-editor and the AAX shell under $Out" | Tee-Object -FilePath $log -Append
    $env:CARGO_TARGET_DIR = Join-Path $Out 'cargo'
    Push-Location -LiteralPath $root
    try { $code = Invoke-Logged 'cargo' @('build', '--release', '--lib', '-p', 'example-editor') } finally { Pop-Location }
    if ($code -ne 0) { [Console]::Error.WriteLine("aax_validate: cargo build failed, see $log"); exit 2 }
    $editor = if ($env:OP_EDITOR) { $env:OP_EDITOR } else { 'ON' }
    $cmakeDir = Join-Path $Out 'cmake'
    $code = Invoke-Logged 'cmake' @('-S', (Join-Path $root 'native'), '-B', $cmakeDir, '-A', 'x64',
        "-DOP_RUST_LIB=$(Join-Path $Out 'cargo\release\example_editor.lib')",
        "-DOP_AAX_SDK=$sdk", '-DOP_AAX_PAGES=ON', '-DOP_BUILD_VST3=OFF', '-DOP_BUILD_AAX=ON', "-DOP_EDITOR=$editor",
        '-DOP_PRODUCT=Example Gain', '-DOP_BUNDLE_ID=com.opengrid.example', '-DOP_VERSION=0.3.1', '-DOP_MANUFACTURER=OpenGrid')
    if ($code -ne 0) { [Console]::Error.WriteLine("aax_validate: cmake configure failed, see $log"); exit 2 }
    $code = Invoke-Logged 'cmake' @('--build', $cmakeDir, '--config', 'Release', '--target', 'OpenPluginAAX', '-j', '2')
    if ($code -ne 0) { [Console]::Error.WriteLine("aax_validate: cmake build failed, see $log"); exit 2 }
    $found = Get-ChildItem -LiteralPath $cmakeDir -Directory -Filter '*.aaxplugin' | Select-Object -First 1
    if ($found) { $Bundle = $found.FullName }
}

if (-not $Bundle -or -not (Test-Path -LiteralPath $Bundle -PathType Container) -or $Bundle -notmatch '\.aaxplugin$') {
    [Console]::Error.WriteLine("aax_validate: not an .aaxplugin bundle folder: $Bundle")
    exit 2
}
$Bundle = (Resolve-Path -LiteralPath $Bundle).Path
"bundle: $Bundle" | Tee-Object -FilePath $log -Append

# Several tests (info.productids and the other DTT scripts) are started through
# the validator's DTT\run_test.bat, which passes the bundle path on unquoted: with
# a space in it they fail with "Could NOT find specified bundle". Such a bundle is
# validated as a copy under Out whose name has no space (the same binary, renamed
# together with its folder, as AAX requires).
if ($Bundle -match '\s') {
    if ($Out -match '\s') {
        [Console]::Error.WriteLine("aax_validate: the bundle path contains a space and so does the output folder ($Out); pass -Out a folder without spaces")
        exit 2
    }
    $name = [IO.Path]::GetFileNameWithoutExtension($Bundle)
    $plain = ($name -replace '\s', '')
    $copyRoot = Join-Path $Out 'validate'
    $copy = Join-Path $copyRoot "$plain.aaxplugin"
    if (Test-Path -LiteralPath $copy) { Remove-Item -LiteralPath $copy -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $copyRoot | Out-Null
    Copy-Item -LiteralPath $Bundle -Destination $copy -Recurse
    Rename-Item -LiteralPath (Join-Path $copy "Contents\x64\$name.aaxplugin") -NewName "$plain.aaxplugin"
    $Bundle = $copy
    "validating a copy without spaces in its path: $Bundle" | Tee-Object -FilePath $log -Append
}

# dsh.exe without options reads its commands from the console and waits forever
# on a file or a pipe, so this uses its application mode (`dsh -a`, what Avid's
# own DTT scripts use): each command is its name on one line, then its
# parameters as a YAML document that ends with "...". The path goes in a
# single-quoted YAML string, where a backslash is just a backslash.
if ($Bundle.Contains("'")) { [Console]::Error.WriteLine('aax_validate: bundle path must not contain a single quote'); exit 2 }
$commands = @('load_dish', '--- aaxval', '...')
if ($env:AAX_VALIDATOR_TESTS) {
    foreach ($t in ($env:AAX_VALIDATOR_TESTS -split '\s+' | Where-Object { $_ })) {
        $commands += @('runtest', '---', "test: $t", "path: '$Bundle'", 'stringformat: yaml', '...')
    }
} else {
    $commands += @('runtests', '---', "path: '$Bundle'", 'stringformat: yaml', '...')
}
$commands += @('exit', '--- ', '...')
$cmds = Join-Path $Out 'aax_validate.dsh'
[IO.File]::WriteAllText($cmds, (($commands -join "`n") + "`n"), (New-Object Text.UTF8Encoding $false))

$raw = Join-Path $Out 'aax_validate.raw.log'
$err = Join-Path $Out 'aax_validate.err.log'
$timeout = if ($env:AAX_VALIDATOR_TIMEOUT) { [int]$env:AAX_VALIDATOR_TIMEOUT } else { 3600 }
"running dsh (timeout ${timeout}s), raw output in $raw" | Tee-Object -FilePath $log -Append
# dsh loads its DLLs and dishes from its own folder.
$process = Start-Process -FilePath $dsh -ArgumentList '-a' -WorkingDirectory $validator -NoNewWindow -PassThru `
    -RedirectStandardInput $cmds -RedirectStandardOutput $raw -RedirectStandardError $err
$handle = $process.Handle  # keeps ExitCode readable after the process ends
if (-not $process.WaitForExit($timeout * 1000)) {
    "aax_validate: dsh still running after ${timeout}s; stopping it" | Tee-Object -FilePath $log -Append
    # The tests run in child processes of dsh; stop the whole tree.
    & taskkill.exe /PID $process.Id /T /F 2>&1 | Out-Null
    $process.WaitForExit()
}
$rc = $process.ExitCode
Get-Content -LiteralPath $raw, $err -ErrorAction SilentlyContinue | Add-Content -LiteralPath $log -Encoding utf8

# Summarize. Each result carries an id and a result_status (E_COMPLETED_PASS,
# E_COMPLETED_FAIL, E_CANCELED, E_LOST, E_TIMEOUT, ...).
$results = @()
$id = ''
foreach ($line in Get-Content -LiteralPath $raw) {
    if ($line -match '^[\s-]*id:\s*(.*)$') { $id = $Matches[1].Trim().Trim('"') }
    if ($line -match 'result_status:\s*(.*)$') {
        $status = $Matches[1].Trim().Trim('"')
        # test.cycle_counts measures DSP plug-ins: it needs an HDX deck, so without
        # one it cannot complete for a native plug-in. Skipped unless AAX_VALIDATOR_DSP=1.
        if ($env:AAX_VALIDATOR_DSP -ne '1' -and $id -eq 'test.cycle_counts' -and $status -match '^(E_LOST|E_ABORTED|E_CANCELED)$') {
            $status = 'SKIPPED_NEEDS_DSP_HARDWARE'
        }
        $results += [pscustomobject]@{ Id = $id; Status = $status }
    }
}
$summary = Join-Path $Out 'aax_validate.summary.txt'
Set-Content -LiteralPath $summary -Value ($results | ForEach-Object { "$($_.Id)`t$($_.Status)" }) -Encoding utf8
$pass = @($results | Where-Object { $_.Status -eq 'E_COMPLETED_PASS' }).Count
$fail = @($results | Where-Object { $_.Status -match '^(E_COMPLETED_FAIL|E_LOST|E_TIMEOUT|E_ABORTED|E_ERROR)$' }).Count
$total = $results.Count
''
"AAX Validator: $total results, $pass passed, $fail failed, $($total - $pass - $fail) skipped"
foreach ($r in $results) { '  {0,-45} {1}' -f $r.Id, $r.Status }
"full log: $log"
if ($total -eq 0) {
    [Console]::Error.WriteLine("aax_validate: no test results found (dsh exit $rc); treating as failure")
    exit 1
}
if ($fail -ne 0) { exit 1 }
exit 0
