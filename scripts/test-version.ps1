param([ValidateSet('x64','arm64')][string]$Architecture = 'x64')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$reader = Join-Path $PSScriptRoot 'version.ps1'
$expected = & $reader
$future = if ($expected -eq '9.8.7') { '9.8.5' } else { '9.8.7' }
$fixture = Join-Path $root ('build/version-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path "$fixture/agent", "$fixture/scripts", "$fixture/assets" -Force | Out-Null
$manifest = Join-Path $fixture 'agent/Cargo.toml'
$native = Join-Path $root $(if ($Architecture -eq 'arm64') { 'build/native-arm64' } else { 'build/native' })
$binary = Join-Path $native 'Connecty.Agent.exe'
if ((& $reader -Binary $binary) -ne $expected) { throw 'Built agent version does not match the manifest' }
function Assert-Rejected([scriptblock]$Work) {
    $rejected = $false
    try { & $Work | Out-Null } catch { $rejected = $true }
    if (!$rejected) { throw 'Invalid or inconsistent version was accepted' }
}
@'
[package]
name = "version-check"
version = "9.8.7" # future release
[dependencies]
unrelated = { version = "1.0.0" }
'@.Replace('9.8.7', $future) | Set-Content -LiteralPath $manifest
if ((& $reader -Manifest $manifest) -ne $future) { throw 'Package version was not read independently of dependencies' }
Assert-Rejected { & $reader -Manifest $manifest -Binary $binary }

# Exercise the real CMake version configuration without compiling a second app.
Copy-Item -LiteralPath $reader -Destination "$fixture/scripts/version.ps1"
Copy-Item -LiteralPath "$root/assets/ConnectyVersion.h.in" -Destination "$fixture/assets/ConnectyVersion.h.in"
$cmake = Get-Content -LiteralPath "$root/CMakeLists.txt" -Raw
$configuration = $cmake.Substring(0, $cmake.IndexOf('if(NOT WIN32')).Replace('LANGUAGES CXX RC', 'LANGUAGES NONE')
$configuration | Set-Content -LiteralPath "$fixture/CMakeLists.txt"
$make = (Select-String -LiteralPath "$native/CMakeCache.txt" -Pattern '^CMAKE_MAKE_PROGRAM:FILEPATH=(.+)$').Matches[0].Groups[1].Value
& cmake -S $fixture -B "$fixture/out" -G Ninja "-DCMAKE_MAKE_PROGRAM=$make"
if ($LASTEXITCODE) { throw 'Future-version CMake configuration failed' }
if ((Get-Content -LiteralPath "$fixture/out/ConnectyVersion.h" -Raw) -notmatch ('#define CONNECTY_VERSION_W L"' + [regex]::Escape($future) + '"')) {
    throw 'C++ version header did not follow the manifest'
}
'[package]', 'version = "9.8.6"' | Set-Content -LiteralPath $manifest
& cmake --build "$fixture/out"
if ($LASTEXITCODE) { throw 'Version edit did not regenerate CMake metadata' }
if ((Get-Content -LiteralPath "$fixture/out/ConnectyVersion.h" -Raw) -notmatch '#define CONNECTY_VERSION_NUMERIC 9,8,6,0') {
    throw 'Windows numeric version did not follow a subsequent version edit'
}
foreach ($invalid in @('1.2', '1.2.3-beta', '65536.0.0', '01.2.3', 'not-a-version')) {
    "[package]`nversion = `"$invalid`"" | Set-Content -LiteralPath $manifest
    Assert-Rejected { & $reader -Manifest $manifest }
}
"[dependencies]`nversion = `"1.0.0`"" | Set-Content -LiteralPath $manifest
Assert-Rejected { & $reader -Manifest $manifest }
"[package]`nversion = `"$expected`"`nversion = `"$expected`"" | Set-Content -LiteralPath $manifest
Assert-Rejected { & $reader -Manifest $manifest }
Assert-Rejected { & "$PSScriptRoot/build-installer.ps1" -Version $future }
Assert-Rejected { & "$PSScriptRoot/make-winget.ps1" -Version $future }

# A normal build refreshes only our package version in Cargo.lock.
$cargo = (Get-Content -LiteralPath "$root/agent/Cargo.toml" -Raw).Replace("version = `"$expected`"", "version = `"$future`"")
$cargo | Set-Content -LiteralPath $manifest
Copy-Item -LiteralPath "$root/agent/Cargo.lock" -Destination "$fixture/agent/Cargo.lock"
New-Item -ItemType Directory -Path "$fixture/agent/src" | Out-Null
'fn main() {}' | Set-Content -LiteralPath "$fixture/agent/src/main.rs"
$before = Get-Content -LiteralPath "$fixture/agent/Cargo.lock" -Raw
& cargo update --manifest-path $manifest --workspace --offline
if ($LASTEXITCODE) { throw 'Version-only lockfile refresh failed' }
$after = Get-Content -LiteralPath "$fixture/agent/Cargo.lock" -Raw
$wanted = [regex]::Replace($before, '(?m)(^name = "connecty-agent"\r?\nversion = ")[^"]+(")', '${1}' + $future + '${2}')
if ($after -ne $wanted) { throw 'Version synchronization unexpectedly changed locked dependencies' }
Write-Output 'PASS version parsing, generated resources, automatic regeneration, lockfile preservation and stale-package rejection'
