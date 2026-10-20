# Validation record — 0.3.1

Verified locally on 2026-10-20:

- Release build, 18 Rust tests and the C++ core check passed; formatting and
  Clippy with warnings denied passed.
- Journal tests cover filtering, persistence, disabled recording, rotation,
  retention cleanup and invalid options. Actual IPC checks verified a custom
  folder, retention/file-size changes, invalid-path rejection and restart persistence.
- Nine live transport/fault checks passed on isolated reverse/local ports,
  including reconnection, injected suspend/resume, host-key rejection and cleanup.
  Persisted records included profile identity and forwarded peer open/close events.
- The per-user installer upgrade completed without elevation or reboot. Current
  profiles and startup preference matched a fresh pre-upgrade snapshot; the
  existing tunnel returned to verified status. Installed history IPC worked.
- Main screen and searchable history were inspected. Toolbar buttons retain
  backgrounds for connection actions; secondary icon/copy controls are subtle.
  Settings uses a smaller inset and a linked Chromatic credit. The original tray
  logo is preserved; only the solid coloured badge is enlarged, without a rim.
- Modal requests reserve a shared slot before asynchronous loading; duplicate
  requests are ignored. History has an icon and both explanatory footer
  paragraphs were removed. Both Copy actions share a clipboard handler that
  flushes content for UI-independent persistence and shows a native, three-second
  success InfoBar only after the write succeeds.
- The main visual tree is constructed on demand for tray-only launches. No
  numerical cold-start speed claim is made. The built-in animated Settings gear
  uses pointer/focus transitions.

Logging defaults to two files of up to 2 MiB each and up to seven days of history.
Cleanup runs on writes/history access; untouched folders are not pruned by a
background timer. The browser shows the latest 256 retained matching records.
It records connection metadata, not destination-service login identities.
The expanded Settings form's save/error flow is implemented with an asynchronous
ContentDialog deferral; its full keyboard, accessibility and display-scale matrix
has not been exercised. Earlier installer lifecycle tests below remain the baseline.

## Transport and installer baseline (0.3.0)

Verified locally on 2026-10-20 on Windows 11 x64. These are smoke and fault
checks, not a 24-hour soak or a clean-machine certification.

| Check | Observed result |
| --- | --- |
| Release build | Native Rust agent and C++/WinUI frontend built successfully; Chromatic metadata verified on both executables |
| Unit checks | 16 Rust tests and the C++ core check passed |
| Static checks | Rust formatting and Clippy with warnings denied passed |
| Dependency audit | 186 locked dependencies scanned; no reported advisories |
| Live SSH | Simultaneous reverse/local routes, actual PC return-path verification and UI-independent operation passed |
| Connection faults | Forced TCP loss, silent blackhole detection, recovery and graceful cleanup passed |
| Host trust | Unknown and changed keys rejected; healthy parallel routes preserved |
| Process/storage faults | Child restart, duplicate launch, clean shutdown and corrupt-profile protection passed |
| UI | Native title bar, larger supplied logo, aligned activity icon, expanded/48px compact rail, labelled route fields and tray panel inspected |
| Installer | Inno Setup 7.1.0 per-user silent install, in-place upgrade, uninstall and reinstall passed without elevation or a reboot |
| Preservation | Profiles remained byte-identical; original login preference and fresh verified tunnel restored |
| Installed startup | The installed task was started in the normal user context; original profile loaded and return path verified |
| WinGet | Version, locale and installer manifests validated with WinGet; publisher/product code matched the actual installed-apps record |
| Packaging | Fresh allow-listed staging excludes unrelated portable-folder files; shared UI runtime is not bundled |

Setup detects the tested Windows App SDK 2.5.1 minimum. Its missing-runtime path
uses a Microsoft download pinned by SHA-256; the downloaded file's Authenticode
signature was verified. The host already had that runtime, so a clean machine's
prerequisite-install path was not exercised. Light/dark background artwork was
inspected; the native installer and app were exercised in the current dark theme
and display scale. A full theme, DPI, screen-reader and monitor matrix remains
untested. Binaries are unsigned.

The same SSH transport passed a physical S3 sleep/resume test in the earlier 0.2
build, with Kernel-Power entry/resume evidence. The 0.3 run exercised the injected
power-recovery path; physical sleep was not repeated for installer/UI changes.
Actual logout/login, Modern Standby and a 24-hour soak remain untested. RSA, proxy
or jump routing, host certificates and external key agents remain unsupported.

Machine-specific snapshots, logs and UI captures stay under ignored `build/` and
are excluded from the source and release assets.
