# Smoke test on a real Windows desktop (used by CI):
# start Poni normally, screenshot the top of the screen while idle, hover the
# island to expand it, screenshot again, and record idle CPU / memory. Also
# checks that the island is really on screen, that its transparent
# surroundings stay click-through, and how smoothly it morphs (the "motion"
# lines Poni logs for its first morphs).
#
# Maximized windows: with a maximized window (Notepad) in front, the resting
# island stays on screen over it (0.2.4 and 0.2.5 stepped aside by default,
# which read as the island vanishing), and still does once it's closed.
#
# -Theme pill runs the same in the Pill theme, where the island floats a gap
# below the top edge: the gap above the pill must take the mouse (a pointer
# thrown at the top edge opens it, as on Classic), while beside and below the
# pill, its shadow included, clicks pass through to what's under it. The open
# card takes the mouse with its margin and the gap above it.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Out = "smoke-out",
    [ValidateSet("classic", "pill")][string]$Theme = "classic"
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
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
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
$pill = $Theme -eq "pill"
# Software renderer: a layered window takes the mouse wherever its pixels
# aren't fully transparent. Pill draws no shadow there, so only the outline's
# anti-aliased edge can take it (Poni ignores a pointer there).
$soft = $env:PONI_RENDERER -eq "software"

# Park the cursor away from the island.
[Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null

# A home of its own per run (the CI runs this once per renderer and theme).
$env:PONI_HOME = Join-Path (Resolve-Path ".").Path "$(Split-Path $Out -Leaf)-home"
if ($pill) {
    New-Item -ItemType Directory -Force -Path $env:PONI_HOME | Out-Null
    '{"theme":"pill"}' | Set-Content -Path (Join-Path $env:PONI_HOME "settings.json") -Encoding ascii
    "theme: pill$(if ($soft) { ' (software renderer)' })" | Tee-Object -FilePath $metrics -Append
}
$p = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 4
Shot "1-idle"

if (-not $pill) {
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
} else {
    # Pill geometry in pixels: the island floats 8 DIP below the top edge.
    $g = [System.Drawing.Graphics]::FromHwnd([IntPtr]::Zero)
    $scale = $g.DpiY / 96.0
    $g.Dispose()
    function Px([double]$dip) { [int][Math]::Round($dip * $scale) }
    $gap = Px 8
    "scale: $scale; gap: $gap px" | Tee-Object -FilePath $metrics -Append

    # The pill is on screen (black just inside its top, above its text) ...
    $inside = $gap + (Px 3)
    $c = PixelAt $cx $inside
    Expect "island-visible" ($c.R -lt 40 -and $c.G -lt 40 -and $c.B -lt 40) "pixel ($cx, $inside) = rgb($($c.R), $($c.G), $($c.B))"
    # Its middle row (a 30-32 DIP pill), where its round ends reach furthest.
    $mid = $gap + (Px 16)
    # ... and the gap above it takes the mouse, up to the very top edge.
    $edge = @(0, 4, ($gap - 1)) | ForEach-Object { "$_=$(ClassAt $cx $_)" }
    Expect "gap-takes-mouse" (($edge | Where-Object { $_ -notlike "*=Poni.Island" }).Count -eq 0) "windows at x $cx, y $($edge -join ', ')"
    $on = ClassAt $cx $mid
    Expect "island-takes-mouse" ($on -eq "Poni.Island") "window at ($cx, $mid) = '$on'"

    # Where the gap stops taking the mouse along the top edge: the pill's ends.
    function EdgeAt([int]$y, [int]$dir, [int]$limit) {
        $x = $cx
        while ([Math]::Abs($x - $cx) -lt $limit -and (ClassAt ($x + $dir) $y) -eq "Poni.Island") { $x += $dir }
        $x
    }
    $limit = Px 500
    $right = EdgeAt 0 1 $limit
    $left = EdgeAt 0 -1 $limit
    $width = $right - $left + 1
    Expect "gap-as-wide-as-pill" ($width -ge (Px 40) -and $width -le (Px 420)) "the top edge takes the mouse from x $left to $right ($width px)"
    # The gap's ends line up with the pill's: at the pill's middle row the
    # mouse stops at the same x (the GPU renderer's hit mask; the software
    # renderer's shadow takes it a little further).
    $bodyRight = EdgeAt $mid 1 $limit
    if (-not $soft) {
        Expect "gap-matches-pill" ([Math]::Abs($bodyRight - $right) -le 2) "the pill's middle row takes the mouse to x $bodyRight, the top edge to $right"
    } else {
        "the pill's middle row takes the mouse to x $bodyRight, the top edge to $right (software: its shadow counts)" | Tee-Object -FilePath $metrics -Append
    }

    # Beside the pill at the top edge, and well beside and below it: through.
    $at = @(
        @(($right + (Px 12)), 0), @(($left - (Px 12)), 0),
        @(($bodyRight + (Px 30)), $mid), @(($left - (Px 30)), $mid),
        @($cx, (Px 120))
    ) | ForEach-Object { "($($_[0]), $($_[1]))=$(ClassAt $_[0] $_[1])" }
    Expect "beside-below-click-through" (($at | Where-Object { $_ -like "*=Poni.Island" }).Count -eq 0) "windows at $($at -join ', ')"
    # On its shadow, just beside and just below the pill: through as well on
    # the GPU renderer (its hit mask leaves the shadow out).
    $bottom = $mid
    while ($bottom - $mid -lt (Px 120) -and (ClassAt $cx ($bottom + 1)) -eq "Poni.Island") { $bottom += 1 }
    $shadow = @(
        @(($right + (Px 3)), $mid), @(($left - (Px 3)), $mid), @($cx, ($bottom + (Px 3)))
    ) | ForEach-Object { "($($_[0]), $($_[1]))=$(ClassAt $_[0] $_[1])" }
    if (-not $soft) {
        Expect "pill-height" (($bottom - $gap) -ge (Px 20) -and ($bottom - $gap) -le (Px 60)) "the pill takes the mouse from y $gap to $bottom"
        Expect "shadow-click-through" (($shadow | Where-Object { $_ -like "*=Poni.Island" }).Count -eq 0) "windows at $($shadow -join ', ')"
    } else {
        "software renderer: the mouse is taken down to y $bottom; on the shadow: $($shadow -join ', ')" | Tee-Object -FilePath $metrics -Append
    }

    # A pointer resting at the top edge beside the pill doesn't open it.
    [Native]::SetCursorPos(($right + (Px 16)), 0) | Out-Null
    Start-Sleep -Milliseconds 120
    [Native]::SetCursorPos(($right + (Px 17)), 0) | Out-Null
    Start-Sleep -Milliseconds 1500
    $shut = ClassAt ($cx + 200) 100
    Expect "beside-does-not-open" ($shut -ne "Poni.Island") "after resting at ($($right + (Px 17)), 0): window at ($($cx + 200), 100) = '$shut'"
    [Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null
    Start-Sleep -Milliseconds 500
}

# Idle CPU over 10 seconds (should be ~0).
$p.Refresh(); $cpu0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 10
$p.Refresh(); $cpu1 = $p.TotalProcessorTime.TotalMilliseconds
$ws = [Math]::Round($p.WorkingSet64 / 1MB, 1)
$priv = [Math]::Round($p.PrivateMemorySize64 / 1MB, 1)
"idle cpu ms over 10s: $([Math]::Round($cpu1 - $cpu0, 1))" | Tee-Object -FilePath $metrics -Append
"working set MB: $ws; private MB: $priv" | Tee-Object -FilePath $metrics -Append

# Hover the island (top centre) to expand it. Pill: thrown at the top edge,
# into the gap above the pill.
$hoverY = if ($pill) { @(0, 1) } else { @(3, 6) }
[Native]::SetCursorPos($cx, $hoverY[0]) | Out-Null
Start-Sleep -Milliseconds 120
[Native]::SetCursorPos($cx, $hoverY[1]) | Out-Null
Start-Sleep -Seconds 2
Shot "2-hover-expanded" 480
$c = PixelAt ($cx + 200) 100
Expect "expanded-visible" ($c.R -lt 40 -and $c.G -lt 40 -and $c.B -lt 40) "pixel ($($cx + 200), 100) = rgb($($c.R), $($c.G), $($c.B))"
$panel = ClassAt ($cx + 200) 100
Expect "expanded-takes-mouse" ($panel -eq "Poni.Island") "window at ($($cx + 200), 100) = '$panel'"
if ($pill) {
    # The open card takes the mouse with the gap above it and its margin
    # (the card is settings.expanded_width, 620 DIP, wide by default).
    $cardRight = $cx + (Px 310)
    $open = @(@(($cx + 200), 0), @(($cardRight + (Px 4)), 100)) | ForEach-Object { "($($_[0]), $($_[1]))=$(ClassAt $_[0] $_[1])" }
    Expect "expanded-gap-and-margin" (($open | Where-Object { $_ -notlike "*=Poni.Island" }).Count -eq 0) "windows at $($open -join ', ')"
    $past = ClassAt ($cardRight + (Px 40)) 100
    Expect "expanded-past-margin-click-through" ($past -ne "Poni.Island") "window at ($($cardRight + (Px 40)), 100) = '$past'"
}

# Leave and let it collapse.
[Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null
Start-Sleep -Seconds 2
Shot "3-collapsed-again"
$after = ClassAt ($cx + 200) 100
Expect "collapsed-click-through" ($after -ne "Poni.Island") "window at ($($cx + 200), 100) = '$after'"

# A few more open/close cycles for the motion log.
for ($i = 0; $i -lt 3; $i++) {
    [Native]::SetCursorPos($cx, $hoverY[0]) | Out-Null
    Start-Sleep -Milliseconds 120
    [Native]::SetCursorPos($cx, $hoverY[1]) | Out-Null
    Start-Sleep -Milliseconds 1500
    [Native]::SetCursorPos($cx, [int]($bounds.Height / 2)) | Out-Null
    Start-Sleep -Milliseconds 1500
}

# A maximized window under the island: it stays there. A click brings the
# window to the front (a window CI starts may not be).
$g = [System.Drawing.Graphics]::FromHwnd([IntPtr]::Zero)
$dpi = $g.DpiY / 96.0
$g.Dispose()
$np = Start-Process -FilePath notepad.exe -WindowStyle Maximized -PassThru
try {
    Start-Sleep -Seconds 2
    $mx = $cx; $my = [int]($bounds.Height / 2)
    [Native]::SetCursorPos($mx, $my) | Out-Null
    [Native]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)  # left down
    [Native]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)  # left up
    Start-Sleep -Seconds 2
    Shot "4-maximized-window"
    # 20 DIPs down: inside the resting island, below the 8 DIP strip a
    # stepped-aside island kept.
    $tab = [int](20 * $dpi)
    $over = ClassAt $cx $tab
    Expect "stays-over-maximized" ($over -eq "Poni.Island") "window at ($cx, $tab) = '$over'"
} finally {
    if (-not $np.HasExited) { Stop-Process -Id $np.Id -Force; $np.WaitForExit(10000) | Out-Null }
}
# The window gone, the island rests where it was.
Start-Sleep -Seconds 2
$back = ClassAt $cx $tab
Expect "back-after-window" ($back -eq "Poni.Island") "window at ($cx, $tab) = '$back'"

$alive = -not $p.HasExited
"still running: $alive" | Tee-Object -FilePath $metrics -Append
if ($alive) { Stop-Process -Id $p.Id -Force; $p.WaitForExit(10000) | Out-Null }

$log = Join-Path $env:PONI_HOME "cache\poni.log"
Copy-Item -Path $log -Destination (Join-Path $Out "poni.log") -ErrorAction SilentlyContinue
if (Test-Path $log) {
    "---- renderer and motion (from poni.log)" | Tee-Object -FilePath $metrics -Append
    Get-Content $log | Where-Object { $_ -match "renderer:|frame clock:|motion |GPU|island (floats|hangs)|step aside:" } | Tee-Object -FilePath $metrics -Append
}
if (-not $alive) { Write-Error "Poni exited during the smoke test"; exit 1 }
if ($failures.Count -gt 0) { Write-Error "smoke checks failed: $($failures -join ', ')"; exit 1 }

# What it costs (size, memory, CPU, wake-ups, at rest and with music playing)
# against tools/weight-budget.json, on the default renderer and theme only
# (the weight run starts Poni in a home of its own, in Classic).
if (-not $env:PONI_RENDERER -and -not $pill) {
    & (Join-Path $PSScriptRoot "ci-weight.ps1") -Exe $Exe -Out (Join-Path $Out "weight")
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
}
