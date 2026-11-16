<#
.SYNOPSIS
Saves a PNG of the desktop, or of one window found by its title.

.DESCRIPTION
Screen capture only works in the interactive desktop session. Started over
SSH (session 0), this script runs itself again through run_in_desktop.ps1,
so it can be called the same way from both places.

With -Title, it waits up to -WaitSec for a visible top-level window whose
title contains the text (or equals it, with -Exact), then captures that
window with PrintWindow (the window's own pixels, even when another window
covers it; a DPI-unaware window comes back at its unscaled size). -FromScreen copies the window's rectangle from the screen
instead, as the user sees it. Without -Title, it captures the whole virtual
desktop (every monitor).

The process is per-monitor DPI aware, so sizes are physical pixels.
Prints the PNG's path and size. Exit 1 when no window matched.

.PARAMETER Path
The PNG to write (its folder is created).

.PARAMETER Title
Text in the window title. Without it, the whole desktop is captured.

.PARAMETER Exact
The title must equal -Title (case-insensitive) rather than contain it.

.PARAMETER WaitSec
How long to wait for the window to appear (default 10).

.PARAMETER FromScreen
Copy the window's area from the screen instead of PrintWindow.

.PARAMETER Foreground
Bring the window to the front first (best effort).

.PARAMETER List
Print the visible top-level windows (handle, process, size, title) and exit.

.EXAMPLE
powershell -NoProfile -File scripts\windows\screenshot.ps1 -Path $env:TEMP\desk.png

.EXAMPLE
powershell -NoProfile -File scripts\windows\screenshot.ps1 -Title "Example Gain" -WaitSec 20 -Path $env:TEMP\editor.png
#>
param(
    [string] $Path,
    [string] $Title,
    [switch] $Exact,
    [int] $WaitSec = 10,
    [switch] $FromScreen,
    [switch] $Foreground,
    [switch] $List
)

$ErrorActionPreference = 'Stop'

if (-not $List -and -not $Path) { throw '-Path is required (or -List)' }

# Session 0 (SSH, services) has no desktop to capture: run in the user's.
if ((Get-Process -Id $PID).SessionId -eq 0) {
    $runner = Join-Path $PSScriptRoot 'run_in_desktop.ps1'
    $a = @()
    if ($Path) {
        if (-not [IO.Path]::IsPathRooted($Path)) { $Path = Join-Path (Get-Location).ProviderPath $Path }
        $a += '-Path', [IO.Path]::GetFullPath($Path)
    }
    if ($Title) { $a += '-Title', $Title }
    if ($Exact) { $a += '-Exact' }
    $a += '-WaitSec', "$WaitSec"
    if ($FromScreen) { $a += '-FromScreen' }
    if ($Foreground) { $a += '-Foreground' }
    if ($List) { $a += '-List' }
    & $runner -Script $PSCommandPath -ArgumentList $a -TimeoutSec ($WaitSec + 60)
    exit $LASTEXITCODE
}

Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class OpShot {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    public delegate bool EnumProc(IntPtr hwnd, IntPtr lParam);

    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern int GetWindowTextLengthW(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hwnd, int attr, out RECT value, int size);

    public static string Title(IntPtr hwnd) {
        int n = GetWindowTextLengthW(hwnd);
        if (n <= 0) return "";
        var sb = new StringBuilder(n + 1);
        GetWindowTextW(hwnd, sb, sb.Capacity);
        return sb.ToString();
    }

    public static List<IntPtr> Visible() {
        var list = new List<IntPtr>();
        EnumWindows((h, l) => { if (IsWindowVisible(h) && GetWindowTextLengthW(h) > 0) list.Add(h); return true; }, IntPtr.Zero);
        return list;
    }

    // The visible frame (DWM extended frame bounds), without the invisible
    // resize borders GetWindowRect includes.
    public static RECT Bounds(IntPtr hwnd) {
        RECT r;
        if (DwmGetWindowAttribute(hwnd, 9, out r, Marshal.SizeOf(typeof(RECT))) != 0) GetWindowRect(hwnd, out r);
        return r;
    }

    [DllImport("user32.dll")] public static extern IntPtr GetWindowDpiAwarenessContext(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);

    // PrintWindow renders at the window's own DPI: a DPI-unaware window comes
    // back at its logical size even though the screen shows it scaled. So the
    // window's own coordinates size the bitmap, and the visible frame (the
    // physical DWM bounds) is scaled into them to crop the resize borders.
    // Returns null when PrintWindow fails.
    public static Bitmap Print(IntPtr hwnd) {
        RECT phys, frame, logical;
        GetWindowRect(hwnd, out phys);
        frame = Bounds(hwnd);
        IntPtr old = SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd));
        Bitmap full;
        bool ok;
        try {
            GetWindowRect(hwnd, out logical);
            int lw = logical.Right - logical.Left, lh = logical.Bottom - logical.Top;
            if (lw <= 0 || lh <= 0) return null;
            full = new Bitmap(lw, lh, PixelFormat.Format24bppRgb);
            using (Graphics g = Graphics.FromImage(full)) {
                IntPtr hdc = g.GetHdc();
                ok = PrintWindow(hwnd, hdc, 2); // PW_RENDERFULLCONTENT
                g.ReleaseHdc(hdc);
            }
        } finally {
            SetThreadDpiAwarenessContext(old);
        }
        if (!ok) { full.Dispose(); return null; }
        double sx = (double)full.Width / Math.Max(1, phys.Right - phys.Left);
        double sy = (double)full.Height / Math.Max(1, phys.Bottom - phys.Top);
        int x = (int)Math.Round((frame.Left - phys.Left) * sx), y = (int)Math.Round((frame.Top - phys.Top) * sy);
        int w = Math.Min(full.Width - x, (int)Math.Round((frame.Right - frame.Left) * sx));
        int h = Math.Min(full.Height - y, (int)Math.Round((frame.Bottom - frame.Top) * sy));
        if (x < 0 || y < 0 || w <= 0 || h <= 0) return full;
        Bitmap cropped = full.Clone(new Rectangle(x, y, w, h), PixelFormat.Format24bppRgb);
        full.Dispose();
        return cropped;
    }
}
'@ -ReferencedAssemblies System.Drawing

# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
[void][OpShot]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Describe([IntPtr] $h) {
    $procId = [uint32]0
    [void][OpShot]::GetWindowThreadProcessId($h, [ref]$procId)
    $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
    $r = [OpShot]::Bounds($h)
    '{0,-10} {1,-20} {2,5}x{3,-5} {4}' -f ('0x{0:X}' -f $h.ToInt64()), $p.ProcessName, ($r.Right - $r.Left), ($r.Bottom - $r.Top), [OpShot]::Title($h)
}

if ($List) {
    foreach ($h in [OpShot]::Visible()) { Describe $h }
    exit 0
}

if (-not [IO.Path]::IsPathRooted($Path)) { $Path = Join-Path (Get-Location).ProviderPath $Path }
$Path = [IO.Path]::GetFullPath($Path)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null

if ($Title) {
    $hwnd = [IntPtr]::Zero
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        foreach ($h in [OpShot]::Visible()) {
            $t = [OpShot]::Title($h)
            if (($Exact -and $t -ieq $Title) -or (-not $Exact -and $t.IndexOf($Title, [StringComparison]::OrdinalIgnoreCase) -ge 0)) { $hwnd = $h; break }
        }
        if ($hwnd -ne [IntPtr]::Zero -or $clock.Elapsed.TotalSeconds -ge $WaitSec) { break }
        Start-Sleep -Milliseconds 200
    }
    if ($hwnd -eq [IntPtr]::Zero) {
        [Console]::Error.WriteLine("screenshot: no visible window titled '$Title' after $WaitSec s. Visible windows:")
        foreach ($h in [OpShot]::Visible()) { [Console]::Error.WriteLine((Describe $h)) }
        exit 1
    }
    if ([OpShot]::IsIconic($hwnd)) { [void][OpShot]::ShowWindow($hwnd, 9) }  # SW_RESTORE
    if ($Foreground) { [void][OpShot]::SetForegroundWindow($hwnd) }
    $r = [OpShot]::Bounds($hwnd)
    $w = $r.Right - $r.Left; $hgt = $r.Bottom - $r.Top
    if ($w -le 0 -or $hgt -le 0) { [Console]::Error.WriteLine("screenshot: window has no area"); exit 1 }
    if ($FromScreen) {
        $bmp = New-Object Drawing.Bitmap $w, $hgt, ([Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $g = [Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    } else {
        $bmp = [OpShot]::Print($hwnd)
        if ($null -eq $bmp) { [Console]::Error.WriteLine("screenshot: PrintWindow failed; try -FromScreen"); exit 1 }
        $w = $bmp.Width; $hgt = $bmp.Height
        $g = $null
    }
    $what = "window '$([OpShot]::Title($hwnd))'"
} else {
    # SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN
    $x = [OpShot]::GetSystemMetrics(76); $y = [OpShot]::GetSystemMetrics(77)
    $w = [OpShot]::GetSystemMetrics(78); $hgt = [OpShot]::GetSystemMetrics(79)
    $bmp = New-Object Drawing.Bitmap $w, $hgt, ([Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $what = 'desktop'
}
if ($g) { $g.Dispose() }
$bmp.Save($Path, [Drawing.Imaging.ImageFormat]::Png)
"$Path ($w x $hgt, $what)"
$bmp.Dispose()
exit 0
