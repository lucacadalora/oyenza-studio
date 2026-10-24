# Smoke test for the Glass theme on a real Windows desktop (used by CI).
#
# Glass copies the screen under the island and leaves the island out of
# screen capture. So with the copy running, a normal screenshot of the
# island's spot must show what's under it (here: the desktop as it was before
# Poni started), while the island still takes the mouse there. If this
# machine can't copy its screen, Glass draws smoked glass and stays in
# screenshots: the log says why, and that is checked instead.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Out = "smoke-glass"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class GlassNative {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
}
"@
[GlassNative]::SetProcessDPIAware() | Out-Null

$metrics = Join-Path $Out "metrics.txt"
$failures = @()

function Expect([string]$name, [bool]$ok, [string]$detail) {
    $line = "check ${name}: $(if ($ok) { 'ok' } else { 'FAILED' }) ($detail)"
    $line | Tee-Object -FilePath $metrics -Append
    if (-not $ok) { $script:failures += $name }
}

# A screenshot of one rectangle, the way any capture tool takes it.
function Grab([int]$x, [int]$y, [int]$w, [int]$h, [string]$name) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $g.Dispose()
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp
}

# Mean absolute difference per channel (0..255) between two same-size shots.
function MeanDiff($a, $b) {
    $rect = New-Object System.Drawing.Rectangle 0, 0, $a.Width, $a.Height
    $fmt = [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
    $da = $a.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, $fmt)
    $db = $b.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, $fmt)
    $n = $da.Stride * $a.Height
    $ba = New-Object byte[] $n
    $bb = New-Object byte[] $n
    [System.Runtime.InteropServices.Marshal]::Copy($da.Scan0, $ba, 0, $n)
    [System.Runtime.InteropServices.Marshal]::Copy($db.Scan0, $bb, 0, $n)
    $a.UnlockBits($da)
    $b.UnlockBits($db)
    $sum = 0
    for ($i = 0; $i -lt $n; $i += 4) {
        $sum += [Math]::Abs([int]$ba[$i] - [int]$bb[$i]) + [Math]::Abs([int]$ba[$i + 1] - [int]$bb[$i + 1]) + [Math]::Abs([int]$ba[$i + 2] - [int]$bb[$i + 2])
    }
    $sum / ($n / 4 * 3)
}

function ClassAt([int]$x, [int]$y) {
    $p = New-Object GlassNative+POINT
    $p.X = $x; $p.Y = $y
    $h = [GlassNative]::WindowFromPoint($p)
    if ($h -eq [IntPtr]::Zero) { return "" }
    $root = [GlassNative]::GetAncestor($h, 2)  # GA_ROOT
    $sb = New-Object System.Text.StringBuilder 256
    [GlassNative]::GetClassName($root, $sb, 256) | Out-Null
    $sb.ToString()
}

$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$cx = [int]($bounds.Width / 2)
[GlassNative]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null

# A home of its own, with the Glass theme picked.
$env:PONI_HOME = Join-Path (Resolve-Path ".").Path "$(Split-Path $Out -Leaf)-home"
New-Item -ItemType Directory -Force -Path $env:PONI_HOME | Out-Null
'{"theme":"glass"}' | Set-Content -Path (Join-Path $env:PONI_HOME "settings.json") -Encoding ascii

# The island's spot before Poni starts, then with Glass on.
$w = 200; $h = 26; $x = $cx - 100; $y = 2
$before = Grab $x $y $w $h "0-before"
$p = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 6
$after = Grab $x $y $w $h "1-glass-on"
$diff = MeanDiff $before $after
"difference at the island's spot: $([Math]::Round($diff, 2)) (0..255)" | Tee-Object -FilePath $metrics -Append

$on = ClassAt $cx 4
Expect "island-takes-mouse" ($on -eq "Poni.Island") "window at ($cx, 4) = '$on'"

$log = Join-Path $env:PONI_HOME "cache\poni.log"
$lines = if (Test-Path $log) { Get-Content $log } else { @() }
"---- glass (from poni.log)" | Tee-Object -FilePath $metrics -Append
$lines | Where-Object { $_ -match 'glass:|renderer:' } | Select-Object -First 12 | Tee-Object -FilePath $metrics -Append
$copyOn = [bool]($lines | Where-Object { $_ -match 'glass: screen copy on' })
if ($copyOn) {
    Expect "glass-hidden-from-capture" ($diff -lt 3) "a screenshot shows the desktop under the island (difference $([Math]::Round($diff, 2)))"
} else {
    "the screen copy didn't start here: smoked glass, which stays in screenshots" | Tee-Object -FilePath $metrics -Append
    Expect "glass-smoked-in-screenshots" ($diff -ge 3) "smoked glass shows up in a screenshot (difference $([Math]::Round($diff, 2)))"
}

$alive = -not $p.HasExited
"still running: $alive" | Tee-Object -FilePath $metrics -Append
if ($alive) { Stop-Process -Id $p.Id -Force }
Copy-Item -Path $log -Destination (Join-Path $Out "poni.log") -ErrorAction SilentlyContinue
if (-not $alive) { $failures += "still-running" }

if ($failures.Count -gt 0) {
    "FAILED: $($failures -join ', ')"
    exit 1
}
"glass smoke ok"
