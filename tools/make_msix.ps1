# Build the Microsoft Store package (MSIX) from a release poni.exe:
#
#   ./tools/make_msix.ps1 -Exe target\release\poni.exe -Version 0.1.2
#
# Writes dist\Oyen-Poni-<version>.msix, unsigned: the Store signs what it ships.
# Needs the Windows SDK (makepri.exe, makeappx.exe) and Python with Pillow.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$OutDir = "dist",
    # Partner Center > Oyen Poni > Product identity. These are stand-ins until
    # the name is reserved; the Store refuses a package whose identity differs.
    [string]$IdentityName = "JatevoLabs.OyenPoni",
    [string]$Publisher = "CN=Jatevo Labs",
    [string]$PublisherDisplayName = "Jatevo Labs"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# The newest x64 copy of a Windows SDK tool.
function Find-SdkTool([string]$name) {
    $bin = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    $tool = Get-ChildItem -Path $bin -Filter $name -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Directory.Name -eq "x64" -and $_.Directory.Parent.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
        Select-Object -First 1
    if (-not $tool) { throw "$name not found under $bin (install the Windows SDK)" }
    $tool.FullName
}

function Invoke-Tool([string]$tool, [string[]]$arguments) {
    & $tool @arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$(Split-Path -Leaf $tool) failed with exit code $LASTEXITCODE" }
}

# The Store wants major.minor.build.0 (it keeps the last number for itself).
$v = [version]$Version
$packageVersion = "$($v.Major).$($v.Minor).$([Math]::Max($v.Build, 0)).0"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$layout = Join-Path $OutDir "msix-layout"
if (Test-Path $layout) { Remove-Item -Recurse -Force $layout }
New-Item -ItemType Directory -Force -Path (Join-Path $layout "Assets") | Out-Null
Copy-Item $Exe (Join-Path $layout "poni.exe")
Copy-Item (Join-Path $root "THIRD-PARTY-NOTICES.txt") (Join-Path $layout "THIRD-PARTY-NOTICES.txt")

# Tile, Start list and taskbar images, drawn from the app icon.
python (Join-Path $root "tools\make_msix_assets.py") (Join-Path $root "crates\poni\res\poni.ico") (Join-Path $layout "Assets") | Out-Host
if ($LASTEXITCODE -ne 0) { throw "make_msix_assets.py failed with exit code $LASTEXITCODE" }

$values = @{
    '$IdentityName$'         = $IdentityName
    '$Publisher$'            = $Publisher
    '$PublisherDisplayName$' = $PublisherDisplayName
    '$Version$'              = $packageVersion
}
$manifest = [IO.File]::ReadAllText((Join-Path $root "installer\msix\AppxManifest.xml"))
foreach ($k in $values.Keys) { $manifest = $manifest.Replace($k, [Security.SecurityElement]::Escape($values[$k])) }
if ($manifest -match '\$[A-Za-z]+\$') { throw "AppxManifest.xml: $($Matches[0]) was not filled in" }
$manifestPath = Join-Path $layout "AppxManifest.xml"
[IO.File]::WriteAllText($manifestPath, $manifest, (New-Object Text.UTF8Encoding $false))

# resources.pri lets Windows pick the right image (scale-*, targetsize-*).
# One language and no resource packs, so the config's <packaging> rules go.
$makepri = Find-SdkTool "makepri.exe"
$priConfig = Join-Path $OutDir "priconfig.xml"
Invoke-Tool $makepri @("createconfig", "/cf", $priConfig, "/dq", "en-US", "/pv", "10.0.0", "/o")
[xml]$cfg = Get-Content -Raw $priConfig
foreach ($node in @($cfg.SelectNodes("/resources/packaging"))) { $null = $node.ParentNode.RemoveChild($node) }
$cfg.Save($priConfig)
Invoke-Tool $makepri @("new", "/pr", $layout, "/cf", $priConfig, "/mn", $manifestPath, "/of", (Join-Path $layout "resources.pri"), "/o")

$msix = Join-Path $OutDir "Oyen-Poni-$Version.msix"
$makeappx = Find-SdkTool "makeappx.exe"
Invoke-Tool $makeappx @("pack", "/d", $layout, "/p", $msix, "/o")
Get-Item $msix | Format-List Name, Length
