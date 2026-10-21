[CmdletBinding()]
param([ValidateSet('x64','arm64')][string]$Architecture='x64')
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot
$installed=Join-Path $env:LOCALAPPDATA 'Programs/Chromatic/Connecty'
if(Test-Path "$installed/Connecty.exe"){throw 'Run this clean-install check only on a disposable Windows runner.'}
if(Get-ScheduledTask -TaskName 'Connecty Agent*' -ErrorAction SilentlyContinue){throw 'Existing Connecty startup configuration must not be changed by this check.'}
$version=& "$PSScriptRoot/version.ps1"
$log=Join-Path $root "build/installer-clean-$Architecture.log"
# -Wait intentionally waits for the process tree, like unattended package tools.
$setup=Start-Process -FilePath "$root/dist/Connecty-Setup-$Architecture.exe" -ArgumentList "/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$log`"" -WindowStyle Hidden -Wait -PassThru
if($setup.ExitCode -ne 0){Get-Content $log -Tail 45; throw "Silent clean installation failed: $($setup.ExitCode)"}
try {
    $null=& "$PSScriptRoot/version.ps1" -Binary @("$installed/Connecty.exe", "$installed/Connecty.Agent.exe")
    $entry=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' | Where-Object DisplayName -eq 'Connecty'
    if($entry.DisplayVersion -ne $version){throw 'Installed-apps version mismatch'}
    foreach($argument in @('--runtime-check','--ui-check')){
        $app=Start-Process -FilePath "$installed/Connecty.exe" -ArgumentList $argument -WindowStyle Hidden -PassThru
        if(!$app.WaitForExit(60000)){$app.Kill(); throw "Installed app timed out: $argument"}
        $app.Refresh()
        if($app.ExitCode -ne 0){throw "Installed app check failed: $argument ($($app.ExitCode))"}
    }
    Write-Output "PASS unattended clean $Architecture install, process-tree exit, version registration and installed XAML startup"
} finally {
    $uninstall=Start-Process -FilePath "$installed/unins000.exe" -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -WindowStyle Hidden -Wait -PassThru
    if($uninstall.ExitCode -ne 0){throw "Silent uninstall failed: $($uninstall.ExitCode)"}
}
