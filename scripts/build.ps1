[CmdletBinding()]
param([switch]$Test, [switch]$Package, [ValidateSet('x64','arm64')][string]$Architecture='x64')
$ErrorActionPreference='Stop'
$hostArch=if([Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq 'Arm64'){'arm64'}else{'x64'}
if($Test -and $Architecture -ne $hostArch){throw 'Run target tests on a Windows machine of the matching architecture.'}
$triple=if($Architecture -eq 'arm64'){'aarch64-pc-windows-msvc'}else{'x86_64-pc-windows-msvc'}

$root=Split-Path $PSScriptRoot
$version=& "$PSScriptRoot/version.ps1"
Push-Location $root
try {
$deps=Join-Path $root 'build/deps'
New-Item -ItemType Directory -Force $deps | Out-Null
# Pin component packages: build tools only; never redistributed with the app.
$packages=@{
  'microsoft.windowsappsdk.foundation'='2.3.9'
  'microsoft.windowsappsdk.winui'='2.3.6'
  'microsoft.windowsappsdk.interactiveexperiences'='2.1.6'
  'microsoft.web.webview2'='1.0.3719.77'
}
$paths=@{}
foreach($id in $packages.Keys){
  $version=$packages[$id]
  $cache=Join-Path $env:USERPROFILE ".nuget/packages/$id/$version"
  if(!(Test-Path "$cache/$id.nuspec")){
    $cache=Join-Path $deps "$id.$version"
    if(!(Test-Path "$cache/$id.nuspec")){
      $zip=Join-Path $deps "$id.$version.zip"
      Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/$id/$version/$id.$version.nupkg" -OutFile $zip
      Expand-Archive -LiteralPath $zip -DestinationPath $cache -Force
      Remove-Item -LiteralPath $zip
    }
  }
  $paths[$id]=$cache.Replace('\','/')
}
$sdkRoot=Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10'
$sdkVersion=Get-ChildItem "$sdkRoot/bin" -Directory | Where-Object {Test-Path "$($_.FullName)/x64/cppwinrt.exe"} | Sort-Object {[version]$_.Name} -Descending | Select-Object -First 1 -ExpandProperty Name
if(!$sdkVersion){throw 'Install the Windows 10/11 SDK with C++/WinRT tools.'}
$generated=Join-Path $deps 'generated'
if(!(Test-Path "$generated/complete.stamp")){
  $args=@('-in',"$($paths['microsoft.windowsappsdk.winui'])/metadata",'-in',"$($paths['microsoft.windowsappsdk.foundation'])/metadata",'-in',"$($paths['microsoft.windowsappsdk.interactiveexperiences'])/metadata/10.0.18362.0",'-in',"$($paths['microsoft.web.webview2'])/lib/Microsoft.Web.WebView2.Core.winmd",'-ref',"$sdkRoot/UnionMetadata/$sdkVersion/Windows.winmd",'-out',$generated)
  & "$sdkRoot/bin/$sdkVersion/x64/cppwinrt.exe" @args
  if($LASTEXITCODE){throw "C++/WinRT projection failed ($LASTEXITCODE)"}
  Set-Content "$generated/complete.stamp" 'ok'
}
@"
set(SDK_FOUNDATION "$($paths['microsoft.windowsappsdk.foundation'])")
set(SDK_WINUI "$($paths['microsoft.windowsappsdk.winui'])")
set(SDK_INTERACTIVE "$($paths['microsoft.windowsappsdk.interactiveexperiences'])")
"@ | Set-Content "$deps/paths.cmake"
$iconScript=Get-Item "$PSScriptRoot/make-icon.ps1"
$iconPath=Join-Path $root 'assets/Connecty.ico'
$logoPath=Join-Path $root 'controlly-logo.png'
if(!(Test-Path $iconPath) -or (Get-Item $iconPath).LastWriteTime -lt $iconScript.LastWriteTime -or (Get-Item $iconPath).LastWriteTime -lt (Get-Item $logoPath).LastWriteTime){
  & $iconScript.FullName
}
$vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs=(& $vswhere -all -latest -products '*' -property installationPath | Select-Object -First 1)
if(!$vs){throw 'Install Visual Studio 2022 C++ desktop tools.'}
$ninja=Join-Path $vs 'Common7/IDE/CommonExtensions/Microsoft/CMake/Ninja'
$env:PATH="$ninja;$env:PATH"
$build=Join-Path $root $(if($Architecture -eq 'x64'){'build/native'}else{'build/native-arm64'})
# Refresh only workspace package metadata after a version edit; dependencies stay locked.
& cargo update --manifest-path "$root/agent/Cargo.toml" --workspace --offline
if($LASTEXITCODE){throw 'Cargo lockfile version synchronization failed'}
& cargo build --manifest-path "$root/agent/Cargo.toml" --release --locked --target $triple
if($LASTEXITCODE){throw 'Rust agent build failed'}
New-Item -ItemType Directory -Force $build | Out-Null
$agentSource=Join-Path $root "agent/target/$triple/release/connecty-agent.exe"
$agentDestination=Join-Path $build 'Connecty.Agent.exe'
if(!(Test-Path -LiteralPath $agentDestination) -or (Get-FileHash -LiteralPath $agentSource).Hash -ne (Get-FileHash -LiteralPath $agentDestination).Hash){
  Copy-Item -LiteralPath $agentSource -Destination $agentDestination -Force
}
$batch=Join-Path $deps "compile-$Architecture.cmd"
@"
@echo off
call "$vs\Common7\Tools\VsDevCmd.bat" -no_logo -arch=$Architecture -host_arch=$hostArch >nul
if errorlevel 1 exit /b 1
cmake -S "$root" -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCONNECTY_DEPS="$deps" -DCONNECTY_ARCH=$Architecture
if errorlevel 1 exit /b 1
cmake --build "$build" --parallel 2
exit /b %errorlevel%
"@ | Set-Content $batch
& $env:ComSpec /d /c $batch
if($LASTEXITCODE){throw "Native build failed ($LASTEXITCODE)"}
if($Test){
  & cargo test --manifest-path "$root/agent/Cargo.toml" --locked --target $triple
  if($LASTEXITCODE){throw 'Rust tests failed'}
  & ctest --test-dir $build --output-on-failure; if($LASTEXITCODE){throw 'Frontend tests failed'}
}
} finally { Pop-Location }
if($Package){
  $dist=Join-Path $root "dist/Connecty-$Architecture"
  New-Item -ItemType Directory -Force $dist | Out-Null
  'Connecty.exe','Connecty.Agent.exe','Microsoft.WindowsAppRuntime.Bootstrap.dll','Connecty.ico','Connecty.png','Connecty-tray.ico','Connecty-live.ico','Connecty-retry.ico','Connecty-error.ico' | ForEach-Object {
    $source=Join-Path $build $_
    $destination=Join-Path $dist $_
    if(!(Test-Path $destination) -or (Get-FileHash -LiteralPath $source).Hash -ne (Get-FileHash -LiteralPath $destination).Hash){
      Copy-Item -LiteralPath $source -Destination $destination -Force
    }
  }
  'README.md','LICENSE','NOTICE','THIRD-PARTY-NOTICES.md','THIRD-PARTY-LICENSES.txt' | ForEach-Object {Copy-Item -LiteralPath "$root/$_" -Destination $dist -Force}
  (Get-Content -LiteralPath "$root/NOTICE" -Raw).Replace('/tree/main', "/tree/v$version") | Set-Content -LiteralPath "$dist/NOTICE"
  (Get-Content -LiteralPath "$root/README.md" -Raw).Replace('assets/Connecty.png','Connecty.png').Replace('assets/showcase.png','https://raw.githubusercontent.com/Has-X/Connecty/main/assets/showcase.png').Replace('connecty-github.png','https://raw.githubusercontent.com/Has-X/Connecty/main/connecty-github.png').Replace('installer/assets/preview-dark.png','https://raw.githubusercontent.com/Has-X/Connecty/main/installer/assets/preview-dark.png') | Set-Content -LiteralPath "$dist/README.md"
  Copy-Item -LiteralPath "$root/docs" -Destination $dist -Recurse -Force
  # Release from a fresh allow-listed staging folder, never from a user's portable folder.
  $stage=Join-Path $root ("build/package-"+[guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $stage | Out-Null
  'Connecty.exe','Connecty.Agent.exe','Microsoft.WindowsAppRuntime.Bootstrap.dll','Connecty.ico','Connecty.png','Connecty-tray.ico','Connecty-live.ico','Connecty-retry.ico','Connecty-error.ico','README.md','LICENSE','NOTICE','THIRD-PARTY-NOTICES.md','THIRD-PARTY-LICENSES.txt' | ForEach-Object {
    Copy-Item -LiteralPath (Join-Path $dist $_) -Destination $stage
  }
  Copy-Item -LiteralPath "$root/docs" -Destination $stage -Recurse
  Set-Content -LiteralPath "$root/build/package-stage-$Architecture.txt" -Value $stage
  if($Architecture -eq 'x64'){Set-Content -LiteralPath "$root/build/package-stage.txt" -Value $stage}
  Compress-Archive -Path "$stage/*" -DestinationPath "$root/dist/Connecty-$Architecture.zip" -Force
  Get-ChildItem $dist | Select-Object Name,Length | Format-Table -AutoSize
}
