[CmdletBinding()]
param(
    [ValidateSet('direct','inno')][string]$Mode,
    [switch]$DisposableRunner,
    [switch]$StandardUserChild
)
$ErrorActionPreference = 'Stop'
if (!$DisposableRunner -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ARCH -ne 'ARM64') {
    throw 'This diagnostic removes runtime packages; use only a disposable native ARM64 GitHub runner.'
}
$root = Split-Path $PSScriptRoot
$output = Join-Path $root 'build/arm64-runtime-diagnostic'
New-Item -ItemType Directory -Force -Path $output | Out-Null
if (!$StandardUserChild) {
    # Match WinGet's unelevated Validator account without disturbing the runner
    # user's provisioned apps or the framework mapped by its desktop shell.
    $account = 'Connecty' + [guid]::NewGuid().ToString('N').Substring(0,8)
    $password = ConvertTo-SecureString ('Cx!' + [guid]::NewGuid().ToString('N')) -AsPlainText -Force
    $credential = [pscredential]::new("$env:COMPUTERNAME\$account", $password)
    New-LocalUser -Name $account -Password $password -AccountNeverExpires | Out-Null
    try {
        Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $account
        icacls.exe $output /grant "${account}:(OI)(CI)M" | Out-Null
        if ($LASTEXITCODE) { throw 'Could not grant diagnostic output access' }
        Start-Service seclogon
        $child = Start-Process -FilePath (Get-Process -Id $PID).Path -Credential $credential -LoadUserProfile `
            -WorkingDirectory $root -ArgumentList "-NoProfile -File `"$PSCommandPath`" -Mode $Mode -DisposableRunner -StandardUserChild" `
            -PassThru -RedirectStandardOutput "$output/standard-user.stdout.txt" -RedirectStandardError "$output/standard-user.stderr.txt"
        if (!$child.WaitForExit(420000)) {
            taskkill.exe /PID $child.Id /T /F
            throw 'Standard-user diagnostic timed out'
        }
        $child.Refresh()
        Get-Content "$output/standard-user.stdout.txt"
        Get-Content "$output/standard-user.stderr.txt"
        if ($child.ExitCode -ne 0) { throw "Standard-user diagnostic failed: $($child.ExitCode)" }
    } finally {
        Remove-LocalUser -Name $account
    }
    exit 0
}
Start-Transcript -Path "$output/transcript.txt"
Write-Host "Diagnostic identity: $([Security.Principal.WindowsIdentity]::GetCurrent().Name)"
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Diagnostic must run without elevation' }
# CreateProcessWithLogon inherits the parent's environment. Resolve profile paths
# from this token's loaded profile instead of using runneradmin's temporary dir.
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$env:USERPROFILE = (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid").ProfileImagePath
$env:LOCALAPPDATA = Join-Path $env:USERPROFILE 'AppData/Local'
$env:APPDATA = Join-Path $env:USERPROFILE 'AppData/Roaming'
$env:TEMP = Join-Path $env:LOCALAPPDATA 'Temp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
Write-Host "Standard-user profile: $env:USERPROFILE; temp: $env:TEMP"
function Run-Bounded([string]$File, [string]$Arguments, [string]$Label, [int]$Seconds = 180) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput "$output/$Label.stdout.txt" -RedirectStandardError "$output/$Label.stderr.txt"
    if (!$process.WaitForExit($Seconds * 1000)) {
        Get-CimInstance Win32_Process | Where-Object {
            $_.Name -match 'runtime|connecty|setup|appinstaller' -or $_.ParentProcessId -eq $process.Id
        } | Select-Object ProcessId, ParentProcessId, Name, CommandLine |
            ConvertTo-Json -Depth 3 | Set-Content "$output/$Label-processes.json"
        taskkill.exe /PID $process.Id /T /F | Out-File "$output/$Label-kill.txt"
        throw "$Label timed out after $Seconds seconds"
    }
    $process.Refresh()
    Write-Host "$Label exit code: $($process.ExitCode)"
    return $process.ExitCode
}
try {
    $version = & "$PSScriptRoot/version.ps1"
    $base = "https://github.com/Has-X/Connecty/releases/download/v$version"
    Invoke-WebRequest "$base/SHA256SUMS.txt" -OutFile "$output/SHA256SUMS.txt"
    foreach ($name in @('Connecty-arm64.zip','Connecty-Setup-arm64.exe')) {
        Invoke-WebRequest "$base/$name" -OutFile "$output/$name"
        $hash = (Get-FileHash "$output/$name" -Algorithm SHA256).Hash
        if (!(Get-Content "$output/SHA256SUMS.txt" | Where-Object { $_ -match "(?i)^$hash\s+\*?$([regex]::Escape($name))$" })) {
            throw "Release checksum mismatch: $name"
        }
    }
    Expand-Archive "$output/Connecty-arm64.zip" "$output/payload"
    $app = (Get-ChildItem "$output/payload" -Filter Connecty.exe -Recurse | Select-Object -First 1).FullName
    if (!$app) { throw 'Release app missing' }
    Get-AppxPackage | Where-Object Name -match 'WindowsAppRuntime' |
        Select-Object Name, Version, Architecture, PackageFullName |
        ConvertTo-Json | Set-Content "$output/packages-before.json"
    # Remove dependents first, only in this disposable runner's current user.
    # The normal CI image already carries 2.x for built-in apps.
    $frameworks = @(Get-AppxPackage | Where-Object {
        $_.Name -like 'Microsoft.WindowsAppRuntime.2*' -and $_.Architecture -eq 'Arm64'
    })
    $frameworkNames = @($frameworks.PackageFullName)
    $dependents = @(Get-AppxPackage | Where-Object {
        @($_.Dependencies | Where-Object PackageFullName -in $frameworkNames).Count -gt 0
    })
    $dependents | Select-Object Name, PackageFullName, NonRemovable |
        ConvertTo-Json | Set-Content "$output/removed-dependents.json"
    foreach ($package in $dependents) { Remove-AppxPackage -Package $package.PackageFullName }
    foreach ($package in $frameworks) {
        if (Get-AppxPackage | Where-Object PackageFullName -eq $package.PackageFullName) {
            Remove-AppxPackage -Package $package.PackageFullName
        }
    }
    $check = Run-Bounded $app '--runtime-check' 'missing-runtime-check' 30
    if ($check -eq 0) { throw 'Runtime is still available; missing-runtime path was not exercised' }
    if ($Mode -eq 'direct') {
        $runtime = (Get-Content "$root/packaging/runtime.json" -Raw | ConvertFrom-Json).runtimes.arm64
        $file = "$output/windowsappruntimeinstall-arm64.exe"
        Invoke-WebRequest $runtime.url -OutFile $file
        if ((Get-FileHash $file).Hash -ne $runtime.sha256) { throw 'Microsoft runtime checksum mismatch' }
        $code = Run-Bounded $file '--quiet' 'runtime'
    } else {
        $code = Run-Bounded "$output/Connecty-Setup-arm64.exe" `
            "/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$output/inno.log`"" 'inno'
    }
    if ($code -notin @(0,3010)) { throw "Installation failed: $code" }
    if ((Run-Bounded $app '--runtime-check' 'installed-runtime-check' 30) -ne 0) {
        throw 'Runtime bootstrap still fails after installation'
    }
    Write-Output "PASS missing-runtime $Mode installation on native ARM64"
} finally {
    Get-AppxPackage | Where-Object Name -match 'WindowsAppRuntime' |
        Select-Object Name, Version, Architecture, PackageFullName |
        ConvertTo-Json | Set-Content "$output/packages-after.json"
    Get-WinEvent -FilterHashtable @{LogName='Microsoft-Windows-AppXDeploymentServer/Operational'; StartTime=(Get-Date).AddMinutes(-12)} `
        -ErrorAction SilentlyContinue | Select-Object TimeCreated, Id, LevelDisplayName, Message |
        ConvertTo-Json -Depth 3 | Set-Content "$output/appx-events.json"
    Stop-Transcript
}
