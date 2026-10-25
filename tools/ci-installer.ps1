# Installer check on a real Windows desktop (used by CI): install the previous release, upgrade it with the new
# NSIS installer through its pages (a screenshot of each), check what landed on disk and in the registry, start
# the app, update it silently the way the app's own Install now does, then uninstall. Screenshots are best effort; the checks decide the exit code.
param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Previous = "",
    [string]$Out = "installer-out"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Native {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, int msg, IntPtr w, IntPtr l);
}
"@
[Native]::SetProcessDPIAware() | Out-Null

$results = [System.Collections.Generic.List[object]]::new()
function Check([string]$name, [bool]$ok, [string]$detail = "") {
    $results.Add([pscustomobject]@{ check = $name; ok = $ok; detail = $detail })
    "{0}  {1}  {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" }), $name, $detail) | Write-Host
}

function Shot([string]$name, [System.Drawing.Rectangle]$r) {
    $bmp = New-Object System.Drawing.Bitmap $r.Width, $r.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# The window's own pixels (PrintWindow), so the runner's console can't cover it.
function WindowShot([string]$name, [IntPtr]$h) {
    $r = New-Object Native+RECT
    if (-not [Native]::GetWindowRect($h, [ref]$r)) { return }
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [Native]::PrintWindow($h, $hdc, 2) | Out-Null  # PW_RENDERFULLCONTENT
    $g.ReleaseHdc($hdc); $g.Dispose()
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

# NSIS's Next / Install / Finish button has id 1; a WM_COMMAND for it needs no keyboard focus.
function ClickNext([IntPtr]$h) {
    [Native]::SendMessage($h, 0x0111, [IntPtr]1, [Native]::GetDlgItem($h, 1)) | Out-Null
}

function WaitFor([scriptblock]$cond, [int]$seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 250 }
    return $false
}

$programs = [Environment]::GetFolderPath("Programs")
$installDir = Join-Path $env:LOCALAPPDATA "Programs\Poni"
$exe = Join-Path $installDir "poni.exe"
$dataDir = Join-Path $env:APPDATA "Poni"
$marker = Join-Path $dataDir "ci-marker.txt"
$uninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Poni"

function PoniEntries {
    $roots = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall", "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall"
    @($roots | ForEach-Object { Get-ChildItem $_ -ErrorAction SilentlyContinue } |
        ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -like "*Poni*" -or $_.DisplayName -like "*Oyenza Studio*" })
}

# 1. The previous release, the way people have it today.
if ($Previous) {
    Start-Process -FilePath $Previous -ArgumentList "/S" -Wait
    Check "previous release installed" (Test-Path $exe) $Previous
    Check "previous Start menu shortcut" (Test-Path (Join-Path $programs "Poni.lnk"))
    # The 0.1.x versions from 0.1.2 on named the shortcut "Oyen Poni": put one there too, the upgrade must remove it.
    $old = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $programs "Oyen Poni.lnk"))
    $old.TargetPath = $exe
    $old.Save()
    New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
    Set-Content -Path $marker -Value "settings and data must survive the upgrade"
}

# 2. The new installer, page by page.
$p = Start-Process -FilePath $Setup -PassThru
$h = [IntPtr]::Zero
WaitFor { $p.Refresh(); if ($p.HasExited) { return $true }; $script:h = $p.MainWindowHandle; $script:h -ne [IntPtr]::Zero } 30 | Out-Null
$shown = $h -ne [IntPtr]::Zero
if ($p.HasExited) { Write-Host "installer exited early, code $($p.ExitCode)" }
if (-not $shown) {
    Get-Process | Where-Object { $_.MainWindowTitle } | ForEach-Object { Write-Host "  window: $($_.ProcessName) [$($_.Id)] '$($_.MainWindowTitle)'" }
    Shot "0-no-window" ([System.Windows.Forms.Screen]::PrimaryScreen.Bounds)
}
Check "installer window opens" $shown "'$($p.MainWindowTitle)'"
$walked = $false
if ($shown) {
    Start-Sleep -Seconds 1
    WindowShot "1-welcome" $h
    ClickNext $h; Start-Sleep -Seconds 2
    WindowShot "2-components" $h
    ClickNext $h; Start-Sleep -Seconds 8  # installs, then moves on to the finish page by itself
    WindowShot "3-finish" $h
    ClickNext $h  # Finish, with "Launch Oyenza Studio now" ticked
    $walked = WaitFor { $p.HasExited } 30
}
if (-not $walked) {
    # The pages didn't take the keys: finish the same upgrade silently so the checks below still run.
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    Start-Process -FilePath $Setup -ArgumentList "/S" -Wait
}
Check "walked through the installer pages" $walked "otherwise installed with /S"

# 3. What the upgrade left behind.
$entry = Get-ItemProperty $uninstKey -ErrorAction SilentlyContinue
Check "Installed apps name" ($entry.DisplayName -eq "Oyenza Studio") "$($entry.DisplayName)"
Check "Installed apps version" ($entry.DisplayVersion -eq $Version) "$($entry.DisplayVersion)"
$entries = PoniEntries
Check "one Installed apps entry" ($entries.Count -eq 1) (($entries | ForEach-Object { $_.DisplayName }) -join ", ")
Check "new Start menu shortcut" (Test-Path (Join-Path $programs "Oyenza Studio.lnk"))
Check "old Start menu shortcut removed" (-not (Test-Path (Join-Path $programs "Poni.lnk")))
if ($Previous) { Check "Oyen Poni shortcut removed" (-not (Test-Path (Join-Path $programs "Oyen Poni.lnk"))) }
$vi = (Get-Item $exe -ErrorAction SilentlyContinue).VersionInfo
Check "poni.exe product name" ($vi.ProductName -eq "Oyenza Studio") "$($vi.ProductName)"
Check "poni.exe version" ($vi.ProductVersion -eq $Version) "$($vi.ProductVersion)"
$notices = Join-Path $installDir "THIRD-PARTY-NOTICES.txt"
Check "license notices installed" (Test-Path $notices) $notices
if ($Previous) { Check "data kept through the upgrade" (Test-Path $marker) $marker }
try {
    [System.Drawing.Icon]::ExtractAssociatedIcon($exe).ToBitmap().Save((Join-Path $Out "exe-icon.png"), [System.Drawing.Imaging.ImageFormat]::Png)
} catch { Write-Host "icon extract failed: $_" }

# 4. The app itself starts and stays up (launched by Finish, or started here).
$running = WaitFor { @(Get-Process poni -ErrorAction SilentlyContinue).Count -gt 0 } 8
if (-not $running) { Start-Process -FilePath $exe; $running = WaitFor { @(Get-Process poni -ErrorAction SilentlyContinue).Count -gt 0 } 8 }
Start-Sleep -Seconds 4
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
Shot "4-island" ([System.Drawing.Rectangle]::new($screen.Left, $screen.Top, $screen.Width, [Math]::Min(240, $screen.Height)))
Shot "5-tray" ([System.Drawing.Rectangle]::new($screen.Right - 520, $screen.Bottom - 120, 520, 120))
Check "app running after install" (@(Get-Process poni -ErrorAction SilentlyContinue).Count -gt 0)
Get-Process poni -ErrorAction SilentlyContinue | Stop-Process -Force

# 5. The app's own update (crates/poni/src/services/update.rs): this Setup again with /S /reopen over a running
#    copy. It closes that copy, leaves Start with Windows as it was (off here), and opens the app again: the
#    finish page's "Launch Oyenza Studio now" never shows in silent mode.
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
Remove-ItemProperty -Path $runKey -Name "Poni" -ErrorAction SilentlyContinue
Start-Process -FilePath $exe
$up = WaitFor { @(Get-Process poni -ErrorAction SilentlyContinue).Count -gt 0 } 8
Start-Sleep -Seconds 2
$before = @(Get-Process poni -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
# WaitForExit waits for the Setup alone; Start-Process -Wait would also wait for the app it reopens.
$u = Start-Process -FilePath $Setup -ArgumentList "/S", "/reopen" -PassThru
$u.WaitForExit()
$reopened = WaitFor { $ids = @(Get-Process poni -ErrorAction SilentlyContinue | ForEach-Object { $_.Id }); $ids.Count -gt 0 -and -not ($ids | Where-Object { $before -contains $_ }) } 15
Check "silent update closes the app and opens it again" ($up -and $reopened) "before: $($before -join ', ')"
Check "silent update leaves Start with Windows off" ($null -eq (Get-ItemProperty -Path $runKey -Name "Poni" -ErrorAction SilentlyContinue))
if ($Previous) { Check "data kept through the silent update" (Test-Path $marker) $marker }
Get-Process poni -ErrorAction SilentlyContinue | Stop-Process -Force

# 6. Uninstall: the app and shortcuts go, settings and data stay.
Start-Process -FilePath (Join-Path $installDir "Uninstall.exe") -ArgumentList "/S" -Wait
$gone = WaitFor { -not (Test-Path $exe) } 30
Check "uninstall removes the app" $gone
Check "uninstall removes the license notices" (WaitFor { -not (Test-Path $notices) } 15)
Check "uninstall removes the Installed apps entry" (-not (Test-Path $uninstKey))
Check "uninstall removes the shortcut" (-not (Test-Path (Join-Path $programs "Oyenza Studio.lnk")))
if ($Previous) { Check "uninstall keeps settings and data" (Test-Path $marker) }

$results | ConvertTo-Json | Set-Content (Join-Path $Out "results.json")
# The window and page checks test this script's automation, not the installer; they don't fail the job.
$failed = @($results | Where-Object { -not $_.ok -and $_.check -notin @("installer window opens", "walked through the installer pages") })
if ($failed.Count -gt 0) { Write-Error "$($failed.Count) installer check(s) failed"; exit 1 }
