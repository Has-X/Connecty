[CmdletBinding()]
param([string]$Compiler, [string]$Version='0.3.2', [ValidateSet('x64','arm64')][string]$Architecture='x64')
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot
if(!$Compiler){$Compiler=Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 7/ISCC.exe'}
if(!(Test-Path -LiteralPath $Compiler)){
  $Compiler=Join-Path ${env:ProgramFiles} 'Inno Setup 7/ISCC.exe'
}
if(!(Test-Path -LiteralPath $Compiler)){throw 'Install Inno Setup 7.1.0 or newer.'}
$compilerVersion=(& $Compiler --version | Select-Object -Last 1).Trim()
if([version]$compilerVersion -lt [version]'7.1.0'){throw 'Inno Setup 7.1.0 or newer is required.'}
$runtime=(Get-Content -LiteralPath "$root/packaging/runtime.json" -Raw | ConvertFrom-Json).runtimes.$Architecture
@"
#define RuntimeUrl "$($runtime.url)"
#define RuntimeSha256 "$($runtime.sha256)"
"@ | Set-Content -LiteralPath "$root/installer/runtime.iss"
$payload=(Get-Content -LiteralPath "$root/build/package-stage-$Architecture.txt" -Raw).Trim()
if(!$payload.StartsWith((Join-Path $root 'build/package-'),[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected installer staging path'}
& $Compiler --quiet "/DAppVersion=$Version" "/DAppArch=$Architecture" "/DPayload=$payload" "/DOutputDir=$root/dist" "$root/installer/Connecty.iss"
if($LASTEXITCODE){throw "Inno Setup failed ($LASTEXITCODE)"}
Get-Item "$root/dist/Connecty-Setup-$Architecture.exe" | Select-Object Name,Length
