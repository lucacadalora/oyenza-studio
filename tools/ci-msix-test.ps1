# CI check of the Microsoft Store package (MSIX), used by store.yml:
# sign a copy with a throwaway certificate, install it, run Poni's self-test
# inside the package (with the package identity the Store copy has), then
# start it the normal way and check it keeps running.
# With -TestCopy, the signed copy, the certificate's public half and the
# scripts in tools/store-test-copy go into that folder: a test copy to
# install on a PC (what only the Store copy can do, such as Notifications).
# Windows PowerShell 5.1: the Appx cmdlets don't load in PowerShell 7.
param(
    [Parameter(Mandatory = $true)][string]$Msix,
    [string]$Out = "msix-out",
    [string]$TestCopy = ""
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$Msix = (Resolve-Path $Msix).Path
$failures = New-Object System.Collections.Generic.List[string]

function Find-SdkTool([string]$name) {
    $bin = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    $tool = Get-ChildItem -Path $bin -Filter $name -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Directory.Name -eq "x64" -and $_.Directory.Parent.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
        Select-Object -First 1
    if (-not $tool) { throw "$name not found under $bin (install the Windows SDK)" }
    $tool.FullName
}

Add-Type -AssemblyName System.IO.Compression.FileSystem, System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
namespace PoniCi {
    [ComImport, Guid("2e941141-7f97-4756-ba1d-9decde894a3d"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IApplicationActivationManager {
        [PreserveSig] int ActivateApplication([MarshalAs(UnmanagedType.LPWStr)] string appUserModelId,
            [MarshalAs(UnmanagedType.LPWStr)] string arguments, int options, out uint processId);
    }
    [ComImport, Guid("45BA127D-10A8-46EA-8AB7-56EA9078943C")]
    class ApplicationActivationManager { }
    public static class Apps {
        // Start a packaged app by its AppUserModelID, the way Start does; returns its process id.
        public static uint Launch(string aumid, string arguments) {
            var manager = (IApplicationActivationManager)new ApplicationActivationManager();
            uint pid;
            int hr = manager.ActivateApplication(aumid, arguments, 0, out pid);
            if (hr < 0) Marshal.ThrowExceptionForHR(hr);
            return pid;
        }
    }
    public static class User32 {
        [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
        [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    }
}
"@
[PoniCi.User32]::SetProcessDPIAware() | Out-Null

function Shot([string]$name, [int]$height = 360) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, ([Math]::Min($height, $b.Height))
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# ---------------------------------------------------------------- install

$zip = [IO.Compression.ZipFile]::OpenRead($Msix)
try {
    $reader = New-Object IO.StreamReader($zip.GetEntry("AppxManifest.xml").Open())
    [xml]$manifest = $reader.ReadToEnd()
    $reader.Dispose()
} finally { $zip.Dispose() }
$identity = $manifest.Package.Identity
"package: $($identity.Name) $($identity.Version), publisher $($identity.Publisher)"

# Allow installing trusted packages from outside the Store.
$unlock = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock"
if (-not (Test-Path $unlock)) { New-Item -Path $unlock | Out-Null }
Set-ItemProperty -Path $unlock -Name AllowAllTrustedApps -Value 1 -Type DWord

# A throwaway certificate whose subject matches the manifest's Publisher.
$cert = New-SelfSignedCertificate -Type Custom -Subject $identity.Publisher -KeyUsage DigitalSignature `
    -FriendlyName "Poni CI (MSIX test)" -CertStoreLocation "Cert:\CurrentUser\My" `
    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
$cer = Join-Path $env:TEMP "poni-ci-msix.cer"
Export-Certificate -Cert $cert -FilePath $cer | Out-Null
Import-Certificate -FilePath $cer -CertStoreLocation "Cert:\LocalMachine\TrustedPeople" | Out-Null

# Sign a copy; the unsigned package stays as built (it is what the Store gets).
$signed = Join-Path $env:TEMP "poni-ci-signed.msix"
Copy-Item $Msix $signed -Force
& (Find-SdkTool "signtool.exe") sign /fd SHA256 /sha1 $cert.Thumbprint $signed | Out-Host
if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }

try {
    Add-AppxPackage -Path $signed
} catch {
    # The VC++ runtime framework the manifest depends on may not be on this machine yet.
    "install needs the VCLibs framework ($($_.Exception.Message.Trim())); fetching it"
    $vclibs = Join-Path $env:TEMP "Microsoft.VCLibs.x64.14.00.Desktop.appx"
    Invoke-WebRequest -Uri "https://aka.ms/Microsoft.VCLibs.x64.14.00.Desktop.appx" -OutFile $vclibs -UseBasicParsing
    Add-AppxPackage -Path $signed -DependencyPath $vclibs
}
if ($TestCopy) {
    New-Item -ItemType Directory -Force -Path $TestCopy | Out-Null
    $name = [IO.Path]::GetFileNameWithoutExtension($Msix)
    Copy-Item $signed (Join-Path $TestCopy "$name-test.msix") -Force
    Copy-Item $cer (Join-Path $TestCopy "oyenza-studio-test.cer") -Force
    Copy-Item (Join-Path $PSScriptRoot "store-test-copy\*") $TestCopy -Force
    "test copy: $((Get-ChildItem $TestCopy).Name -join ', ')"
}
$pkg = Get-AppxPackage -Name $identity.Name
if (-not $pkg) { throw "$($identity.Name) is not installed after Add-AppxPackage" }
"installed: $($pkg.PackageFullName)"
"  at $($pkg.InstallLocation)"
$aumid = "$($pkg.PackageFamilyName)!Poni"

# Park the cursor mid-screen so a hover can't change the scripted states.
$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
[PoniCi.User32]::SetCursorPos([int]($bounds.Width / 2), [int]($bounds.Height / 2)) | Out-Null

# ---------------------------------------------------------------- self-test inside the package

$selftest = Join-Path $Out "selftest"
$p = Get-Process -Id ([PoniCi.Apps]::Launch($aumid, "--selftest `"$selftest`""))
$null = $p.Handle  # keep the handle so ExitCode is available after exit
"self-test: process $($p.Id), $($p.Path)"
if (-not $p.WaitForExit(180000)) {
    Stop-Process -Id $p.Id -Force
    $failures.Add("the packaged self-test did not finish within 180 s")
} elseif ($p.ExitCode -ne 0) {
    $failures.Add("the packaged self-test exited with code $($p.ExitCode)")
}
"self-test exit code: $($p.ExitCode)"

$reportPath = Join-Path $selftest "report.json"
if (Test-Path $reportPath) {
    $report = Get-Content -Raw $reportPath | ConvertFrom-Json
    "report: ok=$($report.ok), $($report.states_passed)/$($report.states_total) states, failed: $($report.failed_checks -join ', ')"
    $startup = @($report.checks | Where-Object { $_.name -eq "start-with-windows" })
    if ($startup.Count -eq 0) {
        $failures.Add("report.json has no start-with-windows check")
    } else {
        "start-with-windows: ok=$($startup[0].ok) ($($startup[0].detail))"
        if (-not "$($startup[0].detail)".StartsWith("packaged")) {
            $failures.Add("the self-test ran without the package identity: $($startup[0].detail)")
        }
    }
    # The Notifications add-on: Windows' listener answers inside the package
    # (the manifest declares userNotificationListener). It asks nothing here.
    $notify = @($report.checks | Where-Object { $_.name -eq "notifications" })
    if ($notify.Count -eq 0) {
        $failures.Add("report.json has no notifications check")
    } else {
        "notifications: ok=$($notify[0].ok) ($($notify[0].detail))"
        if (-not $notify[0].ok -or -not "$($notify[0].detail)".StartsWith("packaged")) {
            $failures.Add("Windows' notification listener didn't answer inside the package: $($notify[0].detail)")
        }
    }
} else {
    $failures.Add("the packaged self-test wrote no report.json")
}
if ($failures.Count -gt 0) {
    "---- poni.log (tail)"
    Get-Content (Join-Path $selftest "home\cache\poni.log") -Tail 80 -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------- normal start

$app = Get-Process -Id ([PoniCi.Apps]::Launch($aumid, ""))
$null = $app.Handle
Start-Sleep -Seconds 6
Shot "packaged-idle"
if ($app.HasExited) {
    $failures.Add("the packaged app quit within 6 s of a normal start (exit code $($app.ExitCode))")
} else {
    "normal start: process $($app.Id) running"
    Stop-Process -Id $app.Id -Force
}
# A packaged app's new files under AppData land in its private folder.
$logs = @(@(
    (Join-Path $env:LOCALAPPDATA "Packages\$($pkg.PackageFamilyName)\LocalCache\Local\Poni\poni.log"),
    (Join-Path $env:LOCALAPPDATA "Poni\poni.log")
) | Where-Object { Test-Path $_ })
if ($logs.Count -eq 0) {
    $failures.Add("no poni.log from the normal start")
} else {
    "log: $($logs[0])"
    Copy-Item $logs[0] (Join-Path $Out "packaged-poni.log")
    if (-not (Select-String -Path $logs[0] -SimpleMatch "running as the Microsoft Store package" -Quiet)) {
        $failures.Add("the normal start did not log that it runs as the Store package")
    }
}

if ($failures.Count -gt 0) {
    "---- FAILED"
    $failures | ForEach-Object { "  $_" }
    exit 1
}
"---- the Store package installs, passes the self-test inside the package and starts"
