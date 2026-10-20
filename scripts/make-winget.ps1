[CmdletBinding()]
param([string]$Version, [ValidateSet('x64','arm64')][string[]]$Architectures=@('x64','arm64'))
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot
$releaseVersion=& "$PSScriptRoot/version.ps1"
if($Version -and $Version -ne $releaseVersion){throw 'Change the release version in agent/Cargo.toml, then rebuild.'}
$Version=$releaseVersion
$null=& "$PSScriptRoot/version.ps1" -Binary @($Architectures | ForEach-Object { "$root/dist/Connecty-Setup-$_.exe" })
$manifest=Join-Path $root "packaging/winget/manifests/c/Chromatic/Connecty/$Version"
New-Item -ItemType Directory -Path $manifest -Force | Out-Null
$installers=foreach($arch in $Architectures){
  $hash=(Get-FileHash -LiteralPath "$root/dist/Connecty-Setup-$arch.exe" -Algorithm SHA256).Hash
  "- Architecture: $arch`n  InstallerUrl: https://github.com/Has-X/Connecty/releases/download/v$Version/Connecty-Setup-$arch.exe`n  InstallerSha256: $hash"
}
$installerRows=$installers -join "`n"
@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.12.0.schema.json
PackageIdentifier: Chromatic.Connecty
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
"@ | Set-Content "$manifest/Chromatic.Connecty.yaml"
@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.12.0.schema.json
PackageIdentifier: Chromatic.Connecty
PackageVersion: $Version
PackageLocale: en-US
Publisher: Chromatic
PublisherUrl: https://chromatic.hu
PublisherSupportUrl: https://github.com/Has-X/Connecty/issues
Author: Chromatic
PackageName: Connecty
PackageUrl: https://github.com/Has-X/Connecty
License: AGPL-3.0-or-later
LicenseUrl: https://github.com/Has-X/Connecty/blob/main/LICENSE
Copyright: Copyright (c) 2026 Chromatic
ShortDescription: Lightweight native Windows SSH tunnel manager with a WinUI 3 interface and an independent Rust agent.
Description: |-
  Manage persistent reverse and local SSH tunnels from a compact Fluent interface.
  The independent Rust agent reconnects automatically and keeps tunnels running
  when the window is closed. Includes key authentication, strict host-key checks,
  named-pipe communication and optional return-path verification.
Moniker: connecty
Tags:
- ssh
- reverse-ssh
- tunnel
- winui
ReleaseNotesUrl: https://github.com/Has-X/Connecty/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: 1.12.0
"@ | Set-Content "$manifest/Chromatic.Connecty.locale.en-US.yaml"
@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.12.0.schema.json
PackageIdentifier: Chromatic.Connecty
PackageVersion: $Version
MinimumOSVersion: 10.0.22000.0
InstallerType: inno
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
InstallerSwitches:
  Silent: /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
  SilentWithProgress: /SP- /SILENT /SUPPRESSMSGBOXES /NORESTART
  InstallLocation: /DIR="<INSTALLPATH>"
  Log: /LOG="<LOGPATH>"
UpgradeBehavior: install
ProductCode: '{C857E631-EF51-4D52-BCE6-9B9475A9CB9F}_is1'
AppsAndFeaturesEntries:
- DisplayName: Connecty
  Publisher: Chromatic
  DisplayVersion: $Version
  ProductCode: '{C857E631-EF51-4D52-BCE6-9B9475A9CB9F}_is1'
ElevationRequirement: elevationProhibited
Installers:
$installerRows
ManifestType: installer
ManifestVersion: 1.12.0
"@ | Set-Content "$manifest/Chromatic.Connecty.installer.yaml"
$files=foreach($arch in $Architectures){"Connecty-Setup-$arch.exe";"Connecty-$arch.zip"}
$sums=foreach($name in $files){"$((Get-FileHash -LiteralPath "$root/dist/$name" -Algorithm SHA256).Hash.ToLowerInvariant())  $name"}
$sums | Set-Content "$root/dist/SHA256SUMS.txt"
Write-Output $manifest
