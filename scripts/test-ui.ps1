[CmdletBinding()]
param([ValidateSet('x64','arm64')][string]$Architecture='x64')
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot
$native=Join-Path $root $(if($Architecture -eq 'x64'){'build/native'}else{'build/native-arm64'})
$exe=Join-Path $native 'Connecty.exe'
function Run-Check([string]$Path,[string]$Arguments){
  $process=Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
  if(!$process.WaitForExit(60000)){$process.Kill();throw "Native validation timed out: $Arguments"}
  $process.Refresh()
  return $process.ExitCode
}
if((Run-Check $exe '--runtime-check') -ne 0){
  $runtime=(Get-Content "$root/packaging/runtime.json" -Raw | ConvertFrom-Json).runtimes.$Architecture
  $bootstrap=Join-Path $root "build/windowsappruntime-$Architecture.exe"
  Invoke-WebRequest $runtime.url -OutFile $bootstrap
  if((Get-FileHash $bootstrap).Hash -ne $runtime.sha256){throw 'Microsoft runtime checksum mismatch'}
  $signature=Get-AuthenticodeSignature $bootstrap
  if($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation'){throw 'Invalid Microsoft runtime signature'}
  $result=Run-Check $bootstrap '--quiet'
  if($result -notin @(0,3010) -or (Run-Check $exe '--runtime-check') -ne 0){throw "Shared runtime installation failed: $result"}
}
$result=Run-Check $exe '--ui-check'
if(Test-Path "$native/ui-check.log"){Get-Content "$native/ui-check.log"}
if($result -ne 0){throw "Native XAML startup validation failed: $result"}
