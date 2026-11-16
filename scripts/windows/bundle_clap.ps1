# SPDX-License-Identifier: MIT
# Builds OpenPluginSDK examples in release and turns each into a Windows
# .clap. On Windows a .clap is the plug-in DLL under another name, so this is
# `cargo build --release` and a copy of target\release\<lib>.dll to
# <Out>\<Name>.clap. The macOS counterpart is scripts/build_plugin.sh (its
# CLAP step). Nothing is installed in a plug-in folder.
#
#   powershell -NoProfile -File scripts\windows\bundle_clap.ps1 [-Example editor,cpp_bridge] [-Out DIR]
#
# -Example  example folders under examples\ (default: every example that
#           exports a CLAP entry: editor, design_editor, cpp_bridge, role_tags)
# -Out      folder that receives the .clap files (default: target\clap)
# -DebugProfile  build the debug profile instead of release
#
# A crate whose [lib] has no cdylib (role_tags: staticlib and rlib) is built
# with `cargo rustc --lib --crate-type cdylib`, which builds that one crate as
# a DLL without changing its manifest.
#
# Prints one line per bundle: `<Name>.clap <- examples\<example> (<package>)`.

[CmdletBinding()]
param(
    [string[]]$Example = @('editor', 'design_editor', 'cpp_bridge', 'role_tags'),
    [string]$Out = '',
    [switch]$DebugProfile
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if ($Out -eq '') { $Out = Join-Path $root 'target\clap' }
$profileDir = if ($DebugProfile) { 'debug' } else { 'release' }
# @(...) keeps a one-element array an array; a bare string would splat as characters.
$profileArgs = @(if (-not $DebugProfile) { '--release' })

# The product name: the folder name in CamelCase (design_editor -> DesignEditor).
function Get-ProductName([string]$folder) {
    ($folder -split '_' | ForEach-Object { if ($_.Length) { $_.Substring(0, 1).ToUpper() + $_.Substring(1) } }) -join ''
}

# One field of the [package] table, and the [lib] crate types, read from the manifest.
function Read-Manifest([string]$path) {
    $section = ''; $name = $null; $types = @()
    foreach ($line in Get-Content -LiteralPath $path) {
        if ($line -match '^\s*\[([^\]]+)\]') { $section = $Matches[1]; continue }
        if ($section -eq 'package' -and $null -eq $name -and $line -match '^\s*name\s*=\s*"([^"]+)"') { $name = $Matches[1] }
        if ($section -eq 'lib' -and $line -match '^\s*crate-type\s*=\s*\[(.*)\]') {
            $types = @([regex]::Matches($Matches[1], '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
        }
    }
    if ($null -eq $name) { throw "no package name in $path" }
    [pscustomobject]@{ Package = $name; CrateTypes = $types }
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

foreach ($ex in $Example) {
    $manifest = Join-Path $root "examples\$ex\Cargo.toml"
    if (-not (Test-Path -LiteralPath $manifest)) { throw "examples\$ex has no Cargo.toml" }
    $m = Read-Manifest $manifest
    $lib = $m.Package -replace '-', '_'
    Push-Location $root
    # cargo reports progress on stderr; Windows PowerShell 5 would turn each line into a
    # terminating error under 'Stop'. The exit code is checked instead.
    $ErrorActionPreference = 'Continue'
    try {
        if ($m.CrateTypes -contains 'cdylib') {
            & cargo build @profileArgs -p $m.Package --lib
        } else {
            & cargo rustc @profileArgs -p $m.Package --lib --crate-type cdylib
        }
        if ($LASTEXITCODE -ne 0) { throw "building $($m.Package) failed ($LASTEXITCODE)" }
    } finally {
        $ErrorActionPreference = 'Stop'
        Pop-Location
    }
    $dll = Join-Path $targetDir "$profileDir\$lib.dll"
    if (-not (Test-Path -LiteralPath $dll)) { throw "$dll was not built" }
    $name = Get-ProductName $ex
    $clap = Join-Path $Out "$name.clap"
    Copy-Item -LiteralPath $dll -Destination $clap -Force
    Write-Output "$clap <- examples\$ex ($($m.Package))"
}
