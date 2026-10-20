$ErrorActionPreference='Stop'
$compiler=Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 7/ISCC.exe'
if(Test-Path -LiteralPath $compiler){return}
$bootstrap=Join-Path ([IO.Path]::GetTempPath()) 'connecty-inno-7.1.0-x64.exe'
Invoke-WebRequest 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe' -OutFile $bootstrap
$signature=Get-AuthenticodeSignature -LiteralPath $bootstrap
if($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Pyrsys'){throw 'Inno Setup publisher verification failed'}
$process=Start-Process -FilePath $bootstrap -ArgumentList '/CURRENTUSER','/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-' -WindowStyle Hidden -Wait -PassThru
if($process.ExitCode -ne 0){throw "Inno Setup installation failed ($($process.ExitCode))"}
if(!(Test-Path -LiteralPath $compiler)){throw 'Inno Setup compiler not found'}
