# Installs the Oyenza Studio test copy from this folder: the Microsoft Store
# package, signed with the throwaway certificate of the CI run that built it,
# to try on a PC what only the Store copy can do (the Notifications add-on).
# README.txt says how to run it and how to go back.
#
# It asks for administrator rights once, only to trust that certificate for
# installing apps (Local Machine > Trusted People). Its private key was thrown
# away with the CI run, so nothing else can ever be signed with it.
# Windows PowerShell 5.1: the Appx cmdlets don't load in PowerShell 7.
param([switch]$TrustOnly)

$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$cer = Join-Path $here "oyenza-studio-test.cer"

if ($TrustOnly) {
    # The elevated half: trust the certificate, nothing else.
    Import-Certificate -FilePath $cer -CertStoreLocation "Cert:\LocalMachine\TrustedPeople" | Out-Null
    exit 0
}

$msix = Get-ChildItem -Path $here -Filter "Oyenza-Studio-*-test.msix" | Select-Object -First 1
if (-not $msix -or -not (Test-Path $cer)) {
    throw "Run this from the unzipped oyenza-studio-test-copy folder: it needs the .msix and oyenza-studio-test.cer next to it."
}
$thumb = (New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 $cer).Thumbprint
if (-not (Test-Path "Cert:\LocalMachine\TrustedPeople\$thumb")) {
    "Windows will ask for administrator rights, to trust the test copy's certificate."
    $p = Start-Process powershell -Verb RunAs -Wait -PassThru -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -TrustOnly"
    if ($p.ExitCode -ne 0 -or -not (Test-Path "Cert:\LocalMachine\TrustedPeople\$thumb")) {
        throw "The certificate wasn't trusted, so Windows won't install the test copy."
    }
}

# One island at a time: the test copy can't start while the usual one runs.
$running = Get-Process -Name poni -ErrorAction SilentlyContinue
if ($running) {
    "Quitting the Oyenza Studio that's running (its settings stay)."
    $running | Stop-Process -Force
}

"Installing $($msix.Name)"
try {
    Add-AppxPackage -Path $msix.FullName -ForceApplicationShutdown
} catch {
    # The Visual C++ runtime the package depends on isn't on every PC yet.
    "It needs Microsoft's Visual C++ runtime package first; fetching it."
    $vclibs = Join-Path $env:TEMP "Microsoft.VCLibs.x64.14.00.Desktop.appx"
    Invoke-WebRequest -Uri "https://aka.ms/Microsoft.VCLibs.x64.14.00.Desktop.appx" -OutFile $vclibs -UseBasicParsing
    Add-AppxPackage -Path $msix.FullName -DependencyPath $vclibs -ForceApplicationShutdown
}
$pkg = Get-AppxPackage -Name "JatevoLabs.OyenzaStudio"
if (-not $pkg) { throw "The test copy didn't install." }
"Installed $($pkg.Version). Starting it."
Start-Process "shell:AppsFolder\$($pkg.PackageFamilyName)!Poni"
""
"Next: open the island, press + (Add-ons), switch Notifications on, and answer Windows' question."
"WhatsApp, Telegram and Phone Link notifications then show on the island."
"To go back to your usual copy: Remove-Test-Copy.ps1 in this folder."
