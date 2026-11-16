# SPDX-License-Identifier: MIT
<#
.SYNOPSIS
Fails when a Windows binary imports the Visual C++ runtime.

.DESCRIPTION
The SDK's Windows binaries link the C and C++ runtime statically (.cargo/config.toml
for Rust, MultiThreaded in native/cmake/windows.cmake for the shells), so none needs
the Visual C++ Redistributable. This script lists the imports of every binary under
-Path with `dumpbin /dependents` and fails on any DLL of that runtime: vcruntime*,
msvcp*, concrt*, vccorlib*, msvcr1*, and the debug Universal CRT ucrtbased.dll. The
Universal CRT itself (ucrtbase.dll, api-ms-win-crt-*) is part of Windows 10 and 11
and is allowed.

-Path takes files and folders. A folder is searched recursively for .dll, .exe,
.clap, .vst3 and .aaxplugin files (a VST3 or AAX bundle folder is searched, its
binary is checked); CMake's own CMakeFiles folders are skipped. A folder with no
binary in it is an error, so a wrong path cannot pass.

-SelfTest is the negative control: it compiles one small DLL with /MD and checks
that this script fails on it, and the same source with /MT and checks that it
passes. Both together, so a check that passes everything, or fails everything,
is caught.

dumpbin and cl come from Visual Studio or its Build Tools; when they are not on
PATH the script loads the developer environment itself (vswhere, vcvars64.bat).
Windows PowerShell 5.1 and PowerShell 7 both run it:

    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\check_imports.ps1 -Path target\release, build-shell
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\check_imports.ps1 -SelfTest

Exit code 0 when every binary is clean (or the self-test passed), 1 otherwise.
#>
[CmdletBinding()]
param(
    [string[]]$Path = @(),
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Die([string]$why) { [Console]::Error.WriteLine("check_imports: $why"); exit 1 }

# The DLLs of the Visual C++ runtime, which only the Redistributable installs.
$forbidden = @('vcruntime*.dll', 'msvcp*.dll', 'concrt*.dll', 'vccorlib*.dll', 'msvcr1*.dll', 'ucrtbased.dll')
function Test-Forbidden([string]$dll) {
    $lower = $dll.ToLowerInvariant()
    foreach ($pattern in $forbidden) { if ($lower -like $pattern) { return $true } }
    return $false
}

function Import-VsEnvironment {
    if ((Get-Command 'dumpbin.exe' -ErrorAction SilentlyContinue) -and (Get-Command 'cl.exe' -ErrorAction SilentlyContinue)) { return }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { Die 'vswhere.exe not found: install Visual Studio or its Build Tools with the C++ workload' }
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1
    if (-not $vs) { Die 'no Visual Studio installation with the C++ tools was found' }
    $vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path -LiteralPath $vcvars)) { Die "$vcvars not found" }
    foreach ($line in (cmd /d /c "`"$vcvars`" >nul 2>&1 && set")) {
        $at = $line.IndexOf('=')
        if ($at -gt 0) { Set-Item -LiteralPath ('Env:' + $line.Substring(0, $at)) -Value $line.Substring($at + 1) }
    }
    foreach ($tool in 'dumpbin.exe', 'cl.exe') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Die "$tool is not in Visual Studio's environment ($vcvars)" }
    }
}

# The DLLs one binary imports, as dumpbin /dependents names them (delay-loaded ones too).
function Get-Imports([string]$file) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $lines = @(& dumpbin.exe /nologo /dependents $file 2>&1 | ForEach-Object { "$_" }) } finally { $ErrorActionPreference = $previous }
    if ($LASTEXITCODE -ne 0) { Die "dumpbin /dependents failed on ${file}: $($lines -join ' ')" }
    return @($lines | ForEach-Object { if ($_ -match '^\s+(\S+\.dll)\s*$') { $Matches[1] } })
}

# The binaries under the given files and folders.
function Get-Binaries([string[]]$roots) {
    $extensions = @('.dll', '.exe', '.clap', '.vst3', '.aaxplugin')
    $found = @()
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { Die "$root does not exist" }
        $item = Get-Item -LiteralPath $root
        if (-not $item.PSIsContainer) { $found += $item.FullName; continue }
        $here = @(Get-ChildItem -LiteralPath $item.FullName -Recurse -File |
            Where-Object { $extensions -contains $_.Extension.ToLowerInvariant() -and $_.FullName -notmatch '[\\/]CMakeFiles[\\/]' } |
            ForEach-Object { $_.FullName })
        if ($here.Count -eq 0) { Die "no binary (.dll, .exe, .clap, .vst3, .aaxplugin) under $root" }
        $found += $here
    }
    return @($found | Sort-Object -Unique)
}

# Checks each binary and returns how many import the Visual C++ runtime. The lines go
# to the host, so the function's output is the count alone.
function Invoke-Check([string[]]$files) {
    $bad = 0
    foreach ($file in $files) {
        $imports = @(Get-Imports $file)
        $hits = @($imports | Where-Object { Test-Forbidden $_ })
        if ($hits.Count) {
            Write-Host "FAILED: $file imports $($hits -join ', ') (the Visual C++ runtime; link it statically)"
            $bad++
        } else {
            Write-Host "ok  $file : $($imports -join ' ')"
        }
    }
    return $bad
}

Import-VsEnvironment

if ($SelfTest) {
    $dir = Join-Path ([IO.Path]::GetTempPath()) ("openplugin-check-imports-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    try {
        # C++ with an exception, a string and a memset of unknown size, so a /MD build
        # imports the runtime's DLLs whatever the compiler inlines.
        $source = Join-Path $dir 'control.cpp'
        Set-Content -LiteralPath $source -Encoding ASCII -Value @'
#include <cstring>
#include <stdexcept>
#include <string>
extern "C" __declspec(dllexport) int control(char* buffer, int size) {
    try {
        std::string text(static_cast<size_t>(size), 'x');
        std::memset(buffer, 0, static_cast<size_t>(size));
        if (size < 0) throw std::runtime_error(text);
        return static_cast<int>(text.size());
    } catch (const std::exception&) {
        return -1;
    }
}
'@
        $failed = 0
        foreach ($case in @(@{ Flag = '/MD'; Clean = $false }, @{ Flag = '/MT'; Clean = $true })) {
            $name = if ($case.Clean) { 'static' } else { 'dynamic' }
            $out = Join-Path $dir "$name.dll"
            $previous = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try {
                $log = @(& cl.exe /nologo /EHsc /O2 $case.Flag /LD $source "/Fe:$out" "/Fo:$(Join-Path $dir "$name.obj")" 2>&1 | ForEach-Object { "$_" })
            } finally { $ErrorActionPreference = $previous }
            if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $out)) { Die "cl $($case.Flag) failed: $($log -join ' ')" }
            Write-Output "-- built with $($case.Flag): $out"
            $bad = Invoke-Check @($out)
            if ($case.Clean -and $bad -ne 0) { Write-Output "SELF-TEST FAILED: the $($case.Flag) control was rejected"; $failed++ }
            elseif (-not $case.Clean -and $bad -eq 0) { Write-Output "SELF-TEST FAILED: the $($case.Flag) control was not caught"; $failed++ }
            elseif ($case.Clean) { Write-Output "self-test: the $($case.Flag) control passes, as it should" }
            else { Write-Output "self-test: the $($case.Flag) control is caught, as it should be" }
        }
    } finally {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($failed) { exit 1 }
    Write-Output 'self-test passed'
    exit 0
}

if ($Path.Count -eq 0) { Die 'give -Path (files or folders), or -SelfTest' }
$files = @(Get-Binaries $Path)
Write-Output "checking $($files.Count) binaries for Visual C++ runtime imports"
$bad = Invoke-Check $files
if ($bad) { Die "$bad of $($files.Count) binaries import the Visual C++ runtime" }
Write-Output "all $($files.Count) binaries are free of the Visual C++ runtime"
exit 0
