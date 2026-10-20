# Windows installation

Connecty is published by **Chromatic**. Its application identifier and WinGet
package identifier are **Chromatic.Connecty**. It supports Windows 11 x64 and ARM64.

The Inno Setup 7.1 installer installs for the current user in
`%LOCALAPPDATA%\Programs\Chromatic\Connecty`. No administrator rights, browser,
.NET runtime or OpenSSH installation are needed. The shared Windows App SDK 2.5.1
runtime is downloaded from Microsoft only if missing. The download is pinned
to SHA-256 in `packaging/runtime.json`; the release tooling verified its Microsoft
Authenticode signature. The runtime is shared rather than bundled into the app.

Upgrades close the old UI, stop the agent gracefully, retain the resolved profile
folder and startup setting, then restart the agent from the installed location.
Failed installation attempts restore the previous agent when possible. Uninstall
stops this installation's agent and removes its own login task. It preserves
profiles, SSH keys and the per-user profile-folder locator. It does not remove
Microsoft's shared runtime or other reverse-SSH tasks.

## Unattended deployment

```powershell
.\Connecty-Setup-x64.exe /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
```

The uninstaller supports the same silent switches. Setup records its version,
publisher, stable application ID and uninstall command in the current user's
Windows installed-apps registry. A custom location can be passed with `/DIR=`.

WinGet manifests are under `packaging/winget/`. They target the published installer
with its exact hash, user scope, silent switches and installed-apps product code.
They are ready for submission; inclusion here does not mean the package is
already indexed in Microsoft's community catalogue.

## Build

Run `scripts/build.ps1 -Test -Package -Architecture x64`, then `scripts/build-installer.ps1 -Architecture x64` with
Inno Setup 7.1.0 or newer. `scripts/make-winget.ps1` generates versioned manifests
and release checksums. `scripts/prepare-inno.ps1` installs the pinned compiler
for CI after checking its publisher. Installer graphics are theme-specific PNGs;
the application continues to use native WinUI controls and system resources.

The app is currently unsigned. Publishing source and checksums does not replace
Authenticode signing or guarantee SmartScreen reputation.

For ARM64, run the build and tests on Windows ARM64 with `-Architecture arm64`
on both scripts. The app, agent and bootstrap DLL are native ARM64. Inno Setup
7.1 supplies an x64 installer bootstrap, which uses Windows 11 ARM64 emulation;
the installed application itself does not. Runtime downloads and hashes are
pinned separately for each architecture. Source builds can cross-compile without
`-Test` when the corresponding MSVC target tools are installed.

Connecty 0.3.1 is AGPL-3.0-or-later. LICENSE, NOTICE and third-party notices are
included in both installers and portable packages. Earlier published versions
retain their original license.
