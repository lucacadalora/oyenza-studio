# Smoke test on a real Windows desktop (used by CI):
# start Poni normally, screenshot the top of the screen while idle, hover the
# island to expand it, screenshot again, and record idle CPU / memory. Also
# checks that the island is really on screen, that its transparent
# surroundings stay click-through, and how smoothly it morphs (the "motion"
# lines Poni logs for its first morphs).
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Out = "smoke-out"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Native {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
}
"@
[Native]::SetProcessDPIAware() | Out-Null

$metrics = Join-Path $Out "metrics.txt"
$failures = @()

function Shot([string]$name, [int]$height = 360) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $h = [Math]::Min($height, $b.Height)
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# Colour of one screen pixel, as DWM composed it.
function PixelAt([int]$x, [int]$y) {
    $bmp = New-Object System.Drawing.Bitmap 1, 1
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $c = $bmp.GetPixel(0, 0)
    $g.Dispose(); $bmp.Dispose()
    $c
}

# Window class of the top-level window that would get a click at (x, y).
function ClassAt([int]$x, [int]$y) {
    $p = New-Object Native+POINT
    $p.X = $x; $p.Y = $y
    $h = [Native]::WindowFromPoint($p)
    if ($h -eq [IntPtr]::Zero) { return "" }
    $root = [Native]::GetAncestor($h, 2)  # GA_ROOT
    $sb = New-Object System.Text.StringBuilder 256
    [Native]::GetClassName($root, $sb, 256) | Out-Null
    $sb.ToString()
}

function Expect([string]$name, [bool]$ok, [string]$detail) {
    $line = "check ${name}: $(if ($ok) { 'ok' } else { 'FAILED' }) ($detail)"
    $line | Tee-Object -FilePath $metrics -Append
    if (-not $ok) { $script:failures += $name }
}

$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
"screen: $($bounds.Width)x$($bounds.Height)" | Tee-Object -FilePath $metrics
$cx = [int]($bounds.Width / 2)

# Park the cursor away from the island.
[Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null

# A home of its own per run (the CI runs this once per renderer).
$env:PONI_HOME = Join-Path (Resolve-Path ".").Path "$(Split-Path $Out -Leaf)-home"
$p = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 4
Shot "1-idle"

# The idle pill is on screen (black at the top centre) and takes the mouse;
# the transparent rest of its window lets clicks through to what's below.
$c = PixelAt $cx 4
Expect "island-visible" ($c.R -lt 40 -and $c.G -lt 40 -and $c.B -lt 40) "pixel ($cx, 4) = rgb($($c.R), $($c.G), $($c.B))"
$on = ClassAt $cx 4
Expect "island-takes-mouse" ($on -eq "Poni.Island") "window at ($cx, 4) = '$on'"
$beside = ClassAt ($cx + 220) 4
Expect "transparent-click-through" ($beside -ne "Poni.Island") "window at ($($cx + 220), 4) = '$beside'"
$below = ClassAt $cx 120
Expect "below-click-through" ($below -ne "Poni.Island") "window at ($cx, 120) = '$below'"

# Idle CPU over 10 seconds (should be ~0).
$p.Refresh(); $cpu0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 10
$p.Refresh(); $cpu1 = $p.TotalProcessorTime.TotalMilliseconds
$ws = [Math]::Round($p.WorkingSet64 / 1MB, 1)
$priv = [Math]::Round($p.PrivateMemorySize64 / 1MB, 1)
"idle cpu ms over 10s: $([Math]::Round($cpu1 - $cpu0, 1))" | Tee-Object -FilePath $metrics -Append
"working set MB: $ws; private MB: $priv" | Tee-Object -FilePath $metrics -Append

# Hover the island (top centre) to expand it.
[Native]::SetCursorPos($cx, 3) | Out-Null
Start-Sleep -Milliseconds 120
[Native]::SetCursorPos($cx, 6) | Out-Null
Start-Sleep -Seconds 2
Shot "2-hover-expanded" 480
$c = PixelAt ($cx + 200) 100
Expect "expanded-visible" ($c.R -lt 40 -and $c.G -lt 40 -and $c.B -lt 40) "pixel ($($cx + 200), 100) = rgb($($c.R), $($c.G), $($c.B))"
$panel = ClassAt ($cx + 200) 100
Expect "expanded-takes-mouse" ($panel -eq "Poni.Island") "window at ($($cx + 200), 100) = '$panel'"

# Leave and let it collapse.
[Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null
Start-Sleep -Seconds 2
Shot "3-collapsed-again"
$after = ClassAt ($cx + 200) 100
Expect "collapsed-click-through" ($after -ne "Poni.Island") "window at ($($cx + 200), 100) = '$after'"

# A few more open/close cycles for the motion log.
for ($i = 0; $i -lt 3; $i++) {
    [Native]::SetCursorPos($cx, 3) | Out-Null
    Start-Sleep -Milliseconds 120
    [Native]::SetCursorPos($cx, 6) | Out-Null
    Start-Sleep -Milliseconds 1500
    [Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null
    Start-Sleep -Milliseconds 1500
}

$alive = -not $p.HasExited
"still running: $alive" | Tee-Object -FilePath $metrics -Append
if ($alive) { Stop-Process -Id $p.Id -Force }

$log = Join-Path $env:PONI_HOME "cache\poni.log"
Copy-Item -Path $log -Destination (Join-Path $Out "poni.log") -ErrorAction SilentlyContinue
if (Test-Path $log) {
    "---- renderer and motion (from poni.log)" | Tee-Object -FilePath $metrics -Append
    Get-Content $log | Where-Object { $_ -match "renderer:|frame clock:|motion |GPU" } | Tee-Object -FilePath $metrics -Append
}
if (-not $alive) { Write-Error "Poni exited during the smoke test"; exit 1 }
if ($failures.Count -gt 0) { Write-Error "smoke checks failed: $($failures -join ', ')"; exit 1 }
