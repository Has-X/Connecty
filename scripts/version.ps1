[CmdletBinding()]
param(
    [string]$Manifest = (Join-Path (Split-Path $PSScriptRoot) 'agent/Cargo.toml'),
    [string[]]$Binary = @()
)
$ErrorActionPreference = 'Stop'
$inPackage = $false
$version = $null
foreach ($line in Get-Content -LiteralPath $Manifest) {
    if ($line -match '^\s*\[(.+)\]\s*$') { $inPackage = $Matches[1] -eq 'package' }
    if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"\s*(?:#.*)?$') {
        if ($null -ne $version) { throw 'Duplicate package version' }
        $version = $Matches[1]
    }
}
if ($version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
    @($version -split '\.' | Where-Object { [long]$_ -gt 65535 }).Count) {
    throw 'Set [package].version in agent/Cargo.toml to major.minor.patch (each at most 65535).'
}
foreach ($path in $Binary) {
    $actual = [version](Get-Item -LiteralPath $path).VersionInfo.ProductVersion
    if ($actual.ToString(3) -ne $version -or $actual.Revision -gt 0) {
        throw "Version mismatch in ${path}: $actual; expected $version. Rebuild before packaging."
    }
}
$version
