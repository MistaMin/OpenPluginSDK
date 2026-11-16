# SPDX-License-Identifier: MIT
# Builds a plugin written with OpenPluginSDK into Windows plug-ins: a CLAP file, a
# VST3 bundle and an AAX bundle, in one go. The macOS packager is
# scripts/build_plugin.sh and the Linux one scripts/build_plugin_linux.sh; this
# script takes the same options where they mean the same thing, spelled the
# PowerShell way (--manifest-path is -ManifestPath). See docs/BUILD.md.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\build_plugin.ps1 `
#       -ManifestPath examples\editor\Cargo.toml -Name "Example Gain" -Version 0.3.1 `
#       -Manufacturer OpenGrid -Editor -Tail -Vst3Sdk C:\path\to\vst3sdk -Out dist
#
# What it makes, in -Out (every option can also be set by the variable in brackets):
#
#   CLAP  <Name>.clap                                        the DLL itself
#   VST3  <Name>.vst3\Contents\x86_64-win\<Name>.vst3        needs -Vst3Sdk
#   AAX   <Name>.aaxplugin\Contents\x64\<Name>.aaxplugin     needs -AaxSdk; unsigned
#
# Plugin
#   -ManifestPath FILE  Cargo.toml of the plugin crate                   [OP_MANIFEST]
#   -Package NAME       cargo package (default: the manifest's)          [OP_PACKAGE]
#   -LibName NAME       library file stem, if not the lib target's name  [OP_LIB_NAME]
#   -Name NAME          product name: file names and the version
#                       resource (default: the package name)             [OP_NAME]
#   -Version X.Y.Z      version, numbers only: a suffix such as -beta is
#                       cut off (default: the manifest's)                [OP_VERSION]
#   -Manufacturer NAME  the vendor, as the plugin reports it: the company
#                       in the version resource (default: OpenPlugin)    [OP_MANUFACTURER]
#   -BundleId ID        bundle identifier prefix; no Windows format reads
#                       it, it is passed on for parity with macOS        [OP_BUNDLE_ID]
#   -Features LIST      cargo features for the plugin crate              [OP_FEATURES]
#
# Formats and options
#   -Formats LIST       comma list of clap,vst3,aax, or "all". Default:
#                       clap, vst3 when a VST3 SDK is given, aax when an
#                       AAX SDK is given                                 [OP_FORMATS]
#   -Editor             the shells host the plugin's editor (OP_EDITOR)  [OP_EDITOR=1]
#   -Tail               the shells report the plugin's tail (OP_TAIL)    [OP_TAIL=1]
#   -AaxPages           write the AAX page table into the bundle         [OP_AAX_PAGES=1]
#   -AllowLib DLL       a DLL the plug-in may import besides the system
#                       ones; repeatable. The Visual C++ runtime cannot
#                       be allowed
#                       (comma list in the variable)                     [OP_ALLOW_LIBS]
#
# Paths
#   -Vst3Sdk DIR        VST3 SDK checkout                                [VST3_SDK]
#   -AaxSdk DIR         AAX SDK checkout (2.8.1)                         [AAX_SDK]
#   -Out DIR            folder that receives the plug-ins (default: .\dist)  [OP_OUT]
#   -WorkDir DIR        build folder for cargo and CMake
#                       (default: .\build\openplugin-windows)            [OP_WORK_DIR]
#   -Jobs N             parallel jobs                                    [OP_JOBS]
#
# Other
#   -DryRun             print every command, run none, create nothing
#   -NoVerify           skip the checks at the end
#
# The checks: every binary is a 64-bit DLL; none holds a path of the build machine
# (the home, workspace, SDK and build folders, as bytes and as UTF-16); each imports
# only system DLLs, and none of the Visual C++ runtime (dumpbin /dependents, through
# check_imports.ps1): the C and C++ runtime is linked statically, so no plug-in needs
# the Visual C++ Redistributable; each exports its
# format's entry points (clap_entry; GetPluginFactory, InitDll, ExitDll; the ACF
# functions); and each carries a version resource with the product name, the version
# and the manufacturer. A failed check fails the run.
#
# It needs Rust (MSVC target), CMake and Visual Studio or its Build Tools with the
# C++ workload: the script finds them with vswhere and loads their environment
# itself. Nothing is installed, signed or copied into a plug-in folder. The AAX
# bundle is unsigned: Pro Tools loads it only after PACE signing.
[CmdletBinding()]
param(
    [string]$ManifestPath = $env:OP_MANIFEST,
    [string]$Package = $env:OP_PACKAGE,
    [string]$LibName = $env:OP_LIB_NAME,
    [string]$Name = $env:OP_NAME,
    [string]$Version = $env:OP_VERSION,
    [string]$Manufacturer = $env:OP_MANUFACTURER,
    [string]$BundleId = $env:OP_BUNDLE_ID,
    [string]$Features = $env:OP_FEATURES,
    [string]$Formats = $env:OP_FORMATS,
    [switch]$Editor,
    [switch]$Tail,
    [switch]$AaxPages,
    [string[]]$AllowLib = @(),
    [string]$Vst3Sdk = $env:VST3_SDK,
    [string]$AaxSdk = $env:AAX_SDK,
    [string]$Out = $env:OP_OUT,
    [string]$WorkDir = $env:OP_WORK_DIR,
    [string]$Jobs = $env:OP_JOBS,
    [switch]$DryRun,
    [switch]$NoVerify
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$sdkRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Die([string]$why) { [Console]::Error.WriteLine("build_plugin: $why"); exit 1 }
function Note([string]$text) { Write-Output "== $text" }
function FullPath([string]$path) {
    if (-not [IO.Path]::IsPathRooted($path)) { $path = Join-Path (Get-Location).ProviderPath $path }
    return [IO.Path]::GetFullPath($path)
}
# Prints a command; in a real run, also runs it and stops at a non-zero exit code.
function Run([string]$exe, [string[]]$arguments) {
    Write-Output ('+ ' + $exe + ' ' + (($arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + $_ + '"' } else { $_ } }) -join ' '))
    if ($DryRun) { return }
    # Native tools write progress to stderr; that is not an error here.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $exe @arguments 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $previous }
    if ($LASTEXITCODE -ne 0) { Die "$exe failed with exit code $LASTEXITCODE" }
}

# ---------- settings ----------
if ($env:OP_EDITOR -eq '1') { $Editor = $true }
if ($env:OP_TAIL -eq '1') { $Tail = $true }
if ($env:OP_AAX_PAGES -eq '1') { $AaxPages = $true }
if ($env:OP_ALLOW_LIBS) { $AllowLib += ($env:OP_ALLOW_LIBS -split ',' | Where-Object { $_ }) }
if (-not $ManifestPath) { Die '-ManifestPath is required' }
if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) { Die "$ManifestPath not found" }
$ManifestPath = FullPath $ManifestPath
if (-not $Out) { $Out = 'dist' }
if (-not $WorkDir) { $WorkDir = 'build\openplugin-windows' }
$Out = FullPath $Out
$WorkDir = FullPath $WorkDir
if (-not $Jobs) { $Jobs = "$([Environment]::ProcessorCount)" }
if (-not $Manufacturer) { $Manufacturer = 'OpenPlugin' }
if (-not $BundleId) { $BundleId = 'com.example.openplugin' }

if (-not $Formats) {
    $list = @('clap')
    if ($Vst3Sdk) { $list += 'vst3' }
    if ($AaxSdk) { $list += 'aax' }
    $Formats = $list -join ','
}
if ($Formats -eq 'all') { $Formats = 'clap,vst3,aax' }
$formatList = @($Formats -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
foreach ($f in $formatList) {
    if ($f -in 'lv2', 'au', 'standalone') { Die "format $f is not built on Windows (Windows builds clap, vst3 and aax)" }
    if ($f -notin 'clap', 'vst3', 'aax') { Die "unknown format $f" }
}
function Has([string]$format) { return $formatList -contains $format }
if ((Has 'vst3') -and -not $Vst3Sdk) { Die 'vst3 needs -Vst3Sdk (or VST3_SDK), a VST3 SDK checkout' }
if ((Has 'aax') -and -not $AaxSdk) { Die 'aax needs -AaxSdk (or AAX_SDK), an AAX SDK checkout' }
if ((Has 'vst3') -and -not (Test-Path -LiteralPath (Join-Path $Vst3Sdk 'public.sdk\source\main\dllmain.cpp'))) { Die "-Vst3Sdk $Vst3Sdk is not a VST3 SDK checkout" }
if ((Has 'aax') -and -not (Test-Path -LiteralPath (Join-Path $AaxSdk 'Interfaces\AAX.h'))) { Die "-AaxSdk $AaxSdk is not an AAX SDK checkout" }
if ($Vst3Sdk) { $Vst3Sdk = FullPath $Vst3Sdk }
if ($AaxSdk) { $AaxSdk = FullPath $AaxSdk }
$needShell = (Has 'vst3') -or (Has 'aax')

# The package, its version and its library come from cargo, which also resolves a
# version inherited from the workspace.
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$metadataText = & cargo metadata --no-deps --format-version 1 --manifest-path $ManifestPath 2>$null
$ErrorActionPreference = $previous
if ($LASTEXITCODE -ne 0 -or -not $metadataText) { Die "cargo metadata failed for $ManifestPath" }
$metadata = ($metadataText -join "`n") | ConvertFrom-Json
$crate = $metadata.packages | Where-Object { (FullPath $_.manifest_path) -ieq $ManifestPath } | Select-Object -First 1
if ($Package) { $crate = $metadata.packages | Where-Object { $_.name -eq $Package } | Select-Object -First 1 }
if (-not $crate) { Die 'the package was not found in the manifest; give -Package' }
$Package = $crate.name
$libTarget = $crate.targets | Where-Object { ($_.kind -contains 'cdylib') -or ($_.kind -contains 'staticlib') } | Select-Object -First 1
if (-not $libTarget) { Die "$Package has no cdylib or staticlib library" }
if (-not $LibName) { $LibName = $libTarget.name -replace '-', '_' }
if ((Has 'clap') -and -not ($libTarget.crate_types -contains 'cdylib')) { Die "clap needs a cdylib: add `"cdylib`" to crate-type in $ManifestPath" }
if ($needShell -and -not ($libTarget.crate_types -contains 'staticlib')) { Die "vst3 and aax need a staticlib: add `"staticlib`" to crate-type in $ManifestPath" }
if (-not $Name) { $Name = $Package }
if ($Name -match '[\\/:*?"<>|]') { Die '-Name must be a file name' }
if (-not $Version) { $Version = $crate.version }
$Version = ($Version -split '-')[0]   # numbers only, as on macOS
if ($Version -notmatch '^\d+(\.\d+){0,2}$') { Die "-Version $Version is not X.Y.Z" }
$workspaceRoot = FullPath $metadata.workspace_root
$manifestDir = Split-Path -Parent $ManifestPath

# ---------- Visual Studio's tools ----------
# rc, dumpbin and the compiler CMake uses come from the developer environment.
function Import-VsEnvironment {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { Die 'vswhere.exe not found: install Visual Studio or its Build Tools with the C++ workload' }
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1
    if (-not $vs) { Die 'no Visual Studio installation with the C++ tools was found' }
    $vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path -LiteralPath $vcvars)) { Die "$vcvars not found" }
    foreach ($line in (cmd /d /c "`"$vcvars`" >nul 2>&1 && set")) {
        $at = $line.IndexOf('=')
        if ($at -gt 0) { Set-Item -LiteralPath ("Env:" + $line.Substring(0, $at)) -Value $line.Substring($at + 1) }
    }
    foreach ($tool in 'rc.exe', 'dumpbin.exe', 'cl.exe') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Die "$tool is not in Visual Studio's environment ($vcvars)" }
    }
}
if (-not $DryRun) { Import-VsEnvironment }

# ---------- environment for the build ----------
$targetDir = Join-Path $WorkDir 'target'
$release = Join-Path $targetDir 'release'
$cmakeDir = Join-Path $WorkDir 'native'
$env:CARGO_BUILD_JOBS = $Jobs
$env:CARGO_TARGET_DIR = $targetDir
# No build path in a binary. Path dependencies usually sit next to the workspace, so
# its parent folder is remapped too. The last matching prefix wins, so the general
# ones come first. The unit separator is how cargo splits flags.
$workspaceParent = Split-Path -Parent $workspaceRoot
if (-not $workspaceParent) { $workspaceParent = $workspaceRoot }
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$remaps = @("$($env:USERPROFILE)=home", "$workspaceParent=deps", "$sdkRoot=openplugin-sdk", "$cargoHome=cargo", "$rustupHome=rustup",
    "$workspaceRoot=workspace", "$manifestDir=plugin", "$WorkDir=build")
# CARGO_ENCODED_RUSTFLAGS replaces the target rustflags of .cargo/config.toml, and a
# plugin outside this repository does not read that file, so the static C runtime
# (+crt-static, as the shells' MultiThreaded) is asked for here too.
$rustFlags = @('-C', 'target-feature=+crt-static') + @($remaps | ForEach-Object { "--remap-path-prefix=$_" })
$env:CARGO_ENCODED_RUSTFLAGS = $rustFlags -join [string][char]0x1f
# The folders no shipped binary may name.
$private = @($WorkDir, $workspaceParent, $manifestDir, $sdkRoot, $env:USERPROFILE)
if ($Vst3Sdk) { $private += $Vst3Sdk }
if ($AaxSdk) { $private += $AaxSdk }
$private = @($private | Sort-Object -Unique)

Note "plugin $Name ($Package) $Version by $Manufacturer, formats: $($formatList -join ','), x64 Windows"
if ($DryRun) { Note 'dry run: nothing below is executed' }
if (-not $DryRun) { New-Item -ItemType Directory -Force -Path $WorkDir, $Out | Out-Null }

# ---------- the version resource ----------
# A resource string: a quote is doubled, a backslash escaped.
function RcText([string]$text) { return ($text -replace '\\', '\\') -replace '"', '""' }
$numbers = @($Version -split '\.') + @('0', '0', '0')
$versionCommas = "$($numbers[0]),$($numbers[1]),$($numbers[2]),0"
# Compiles native\win\version.rc.in for one binary and returns the .res file.
function New-VersionResource([string]$description, [string]$fileName, [string]$stem) {
    $rc = Join-Path $WorkDir "version\$stem.rc"
    $res = Join-Path $WorkDir "version\$stem.res"
    Write-Output "+ write $rc" | Out-Host
    if (-not $DryRun) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $rc) | Out-Null
        $text = [IO.File]::ReadAllText((Join-Path $sdkRoot 'native\win\version.rc.in'))
        $values = @{ OP_RC_VERSION_COMMAS = $versionCommas; OP_RC_VERSION = (RcText $Version); OP_RC_COMPANY = (RcText $Manufacturer)
            OP_RC_PRODUCT = (RcText $Name); OP_RC_DESCRIPTION = (RcText $description); OP_RC_FILE = (RcText $fileName) }
        foreach ($key in $values.Keys) { $text = $text.Replace("@$key@", $values[$key]) }
        [IO.File]::WriteAllText($rc, $text, (New-Object Text.UTF8Encoding $false))
    }
    Run 'rc.exe' @('/nologo', '/fo', $res, $rc) | Out-Host
    return $res
}

# ---------- Rust ----------
Note 'Rust library'
$cargoArgs = @('rustc', '--release', '--manifest-path', $ManifestPath, '-p', $Package, '--lib')
if ($Features) { $cargoArgs += @('--features', $Features) }
# The linker arguments reach the DLL only (a static library is not linked): its
# version resource, and the PDB named without its folder.
$cargoArgs += @('--', '-C', 'link-arg=/PDBALTPATH:%_PDB%')
if (Has 'clap') {
    $clapRes = New-VersionResource "$Name (CLAP)" "$Name.clap" 'clap'
    $cargoArgs += @('-C', "link-arg=$clapRes")
}
Run 'cargo' $cargoArgs
$dll = Join-Path $release "$LibName.dll"
$lib = Join-Path $release "$LibName.lib"
if (-not $DryRun) {
    if ((Has 'clap') -and -not (Test-Path -LiteralPath $dll)) { Die "$dll was not built: is -LibName right?" }
    if ($needShell -and -not (Test-Path -LiteralPath $lib)) { Die "$lib was not built: is -LibName right?" }
}

# ---------- plug-ins ----------
$shipped = @()   # every binary shipped, with its format, for the checks
# Replaces one bundle in Out. Only a path directly inside Out is ever removed.
function Place([string]$from, [string]$to) {
    if ((Split-Path -Parent $to) -ne $Out) { Die "refusing to replace ${to}: it is not directly inside $Out" }
    Write-Output "+ replace $to with $from"
    if ($DryRun) { return }
    if (Test-Path -LiteralPath $to) { Remove-Item -LiteralPath $to -Recurse -Force }
    Copy-Item -LiteralPath $from -Destination $to -Recurse
}

if (Has 'clap') {
    Note 'CLAP'
    $bundle = Join-Path $Out "$Name.clap"
    Place $dll $bundle
    $shipped += [pscustomobject]@{ Kind = 'clap'; Path = $bundle }
}
if ($needShell) {
    Note 'VST3 and AAX shells'
    $onOff = { param($flag) if ($flag) { 'ON' } else { 'OFF' } }
    $configure = @('-S', (Join-Path $sdkRoot 'native'), '-B', $cmakeDir, '-A', 'x64', '-UOP_RUST_NATIVE_LIBS',
        "-DOP_RUST_LIB=$lib", "-DOP_PRODUCT=$Name", "-DOP_BUNDLE_ID=$BundleId", "-DOP_VERSION=$Version", "-DOP_MANUFACTURER=$Manufacturer",
        "-DOP_BUILD_VST3=$(& $onOff (Has 'vst3'))", "-DOP_BUILD_AAX=$(& $onOff (Has 'aax'))",
        "-DOP_EDITOR=$(& $onOff $Editor)", "-DOP_TAIL=$(& $onOff $Tail)", "-DOP_AAX_PAGES=$(& $onOff ($AaxPages -and (Has 'aax')))",
        "-DOP_TRIM_PATHS=$(($private | ForEach-Object { $_ -replace '\\', '/' }) -join ';')")
    if (Has 'vst3') { $configure += "-DOP_VST3_SDK=$Vst3Sdk" }
    if (Has 'aax') { $configure += "-DOP_AAX_SDK=$AaxSdk" }
    Run 'cmake' $configure
    # Only the bundles, not the SDK's own proofs.
    $targets = @()
    if (Has 'vst3') { $targets += 'OpenPluginVST3' }
    if (Has 'aax') { $targets += 'OpenPluginAAX' }
    Run 'cmake' (@('--build', $cmakeDir, '--config', 'Release', '-j', $Jobs, '--target') + $targets)
    if (Has 'vst3') {
        Note 'VST3'
        $bundle = Join-Path $Out "$Name.vst3"
        Place (Join-Path $cmakeDir "$Name.vst3") $bundle
        $shipped += [pscustomobject]@{ Kind = 'vst3'; Path = (Join-Path $bundle "Contents\x86_64-win\$Name.vst3") }
    }
    if (Has 'aax') {
        Note 'AAX'
        $bundle = Join-Path $Out "$Name.aaxplugin"
        Place (Join-Path $cmakeDir "$Name.aaxplugin") $bundle
        $shipped += [pscustomobject]@{ Kind = 'aax'; Path = (Join-Path $bundle "Contents\x64\$Name.aaxplugin") }
    }
}

# ---------- checks ----------
$bad = 0
function Fail([string]$text) { Write-Output "FAILED: $text"; $script:bad++ }
if (-not $NoVerify -and -not $DryRun) {
    Note 'checks'
    # What a plug-in may import: Windows' own DLLs and the Universal CRT. The Visual
    # C++ runtime (vcruntime*, msvcp*, concrt*) is linked statically and never allowed,
    # not even with -AllowLib: check_imports.ps1 fails on it.
    $system = @('kernel32.dll', 'ntdll.dll', 'user32.dll', 'gdi32.dll', 'shell32.dll', 'comdlg32.dll', 'comctl32.dll', 'advapi32.dll',
        'ole32.dll', 'oleaut32.dll', 'ws2_32.dll', 'userenv.dll', 'bcrypt.dll', 'bcryptprimitives.dll', 'dbghelp.dll', 'imm32.dll', 'version.dll',
        'ucrtbase.dll') + @($AllowLib | ForEach-Object { $_.ToLowerInvariant() })
    $exports = @{
        clap = @('clap_entry')
        vst3 = @('GetPluginFactory', 'InitDll', 'ExitDll')
        aax  = @('ACFRegisterPlugin', 'ACFRegisterComponent', 'ACFGetClassFactory', 'ACFCanUnloadNow', 'ACFStartup', 'ACFShutdown', 'ACFGetSDKVersion')
    }
    $latin = [Text.Encoding]::GetEncoding(28591)
    foreach ($item in $shipped) {
        $file = $item.Path
        Write-Output "-- $file"
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { Fail "$file is missing"; continue }
        $bytes = [IO.File]::ReadAllBytes($file)
        $headers = (& dumpbin.exe /nologo /headers $file) -join "`n"
        if ($headers -notmatch '8664 machine \(x64\)' -or $headers -notmatch '(?m)^\s+DLL\s*$') { Fail "$file is not a 64-bit DLL" } else { Write-Output "x64 DLL, $($bytes.Length) bytes" }
        # No folder of the build machine, as bytes or as UTF-16, with either slash.
        $narrow = $latin.GetString($bytes)
        $wide = [Text.Encoding]::Unicode.GetString($bytes, 0, $bytes.Length - ($bytes.Length % 2))
        $wideOdd = [Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1 - (($bytes.Length - 1) % 2))
        foreach ($folder in $private) {
            foreach ($spelling in @($folder, ($folder -replace '\\', '/')) | Sort-Object -Unique) {
                foreach ($text in $narrow, $wide, $wideOdd) {
                    $at = $text.IndexOf($spelling, [StringComparison]::OrdinalIgnoreCase)
                    if ($at -ge 0) {
                        $shown = ($text.Substring($at, [Math]::Min(100, $text.Length - $at)) -replace '[^\x20-\x7e]', '.')
                        Fail "path leak: $file contains $spelling ($shown)"
                        break
                    }
                }
            }
        }
        # Imports: no Visual C++ runtime (check_imports.ps1, which the CI jobs run too),
        # then system DLLs only.
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'check_imports.ps1') -Path $file
        if ($LASTEXITCODE -ne 0) { Fail "Visual C++ runtime: $file imports it; the C and C++ runtime must be linked statically" }
        $imports = @(& dumpbin.exe /nologo /dependents $file | ForEach-Object { if ($_ -match '^\s+(\S+\.dll)\s*$') { $Matches[1] } })
        foreach ($import in $imports) {
            $lower = $import.ToLowerInvariant()
            if ($system -contains $lower -or $lower -like 'api-ms-win-*.dll') { continue }
            Fail "unexpected DLL: $file imports $import (allow it with -AllowLib $import)"
        }
        Write-Output "imports $($imports -join ' ')"
        # The format's entry points.
        $exported = @(& dumpbin.exe /nologo /exports $file | ForEach-Object { if ($_ -match '^\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+(\S+)') { $Matches[1] } })
        $missing = @($exports[$item.Kind] | Where-Object { $exported -notcontains $_ })
        if ($missing.Count) { Fail "missing export: $file does not export $($missing -join ', ')" } else { Write-Output "exports $($exports[$item.Kind] -join ' ')" }
        # The version resource.
        $info = (Get-Item -LiteralPath $file).VersionInfo
        Write-Output "version resource: '$($info.ProductName)' $($info.ProductVersion) by '$($info.CompanyName)' ($($info.FileDescription); file version $($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart))"
        if ($info.ProductName -ne $Name) { Fail "${file}: product name '$($info.ProductName)', expected '$Name'" }
        if ($info.ProductVersion -ne $Version -or $info.FileVersion -ne $Version) { Fail "${file}: version '$($info.ProductVersion)' / '$($info.FileVersion)', expected '$Version'" }
        if ("$($info.FileMajorPart),$($info.FileMinorPart),$($info.FileBuildPart),0" -ne $versionCommas) { Fail "${file}: file version numbers are not $versionCommas" }
        if ($info.CompanyName -ne $Manufacturer) { Fail "${file}: company '$($info.CompanyName)', expected '$Manufacturer'" }
    }
    if (Has 'vst3') {
        $licence = Join-Path $Out "$Name.vst3\Contents\Resources\VST3-SDK-LICENSE.txt"
        if (-not (Test-Path -LiteralPath $licence)) { Fail "$licence is missing" }
    }
    if ((Has 'aax') -and $AaxPages) {
        $pages = Join-Path $Out "$Name.aaxplugin\Contents\Resources\${Name}Pages.xml"
        if (-not (Test-Path -LiteralPath $pages)) { Fail "$pages is missing" }
    }
}

if ($bad -ne 0) { Die "$bad check(s) failed" }
Note 'done'
foreach ($item in $shipped) { Write-Output $item.Path }
exit 0
