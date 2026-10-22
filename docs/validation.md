# Validation record — 0.3.5

## Outage and tray recovery (2026-10-22)

- The reported remote-port rejection was reproduced with stale SSH sessions
  retaining both listeners after the carrier changed. Only the identified old
  listener-owning sessions were removed during host recovery. The application
  never kills a remote port owner. Server-side dead-client detection was enabled
  for the tunnel accounts; an isolated silent-drop test with client keepalives
  disabled released its listener after 116.9 seconds.
- The destination Windows OpenSSH service had also stopped with error 1067.
  Restarting it restored authenticated nested logins through both routes. Its
  separate Windows service recovery policy now restarts it after failure.
- The UI access violation was reproduced by delivering `WM_CANCELMODE` during
  cold tray construction. The window was destroyed while `ShowPopup` still used
  its controls. An opening guard and shutdown guard prevent that reentrancy;
  the same native lifecycle check passes with the fix. It also exercises dynamic
  tray rows and ignores a queued reopen after shutdown.
- Rejected reverse requests now disconnect their new session without probing
  the occupied listener. The live regression checks that an existing owner stays
  connected and its listener remains usable.

## Previous release — 0.3.4

## Network-change and installer regressions

Interface-change notifications no longer cancel an established transport or its
forwarded streams. SSH keepalives still detect real outages; connection attempts,
backoff and power recovery still react to native events. A focused regression
test holds pending work across a network hint, then verifies prompt suspension.
The live suite injects a network hint and checks session identity and retries.

Installer recovery now launches through Task Scheduler instead of leaving a
persistent child beneath Setup. A local unattended upgrade waited for the full
process tree to exit, preserved byte-identical profiles and the login preference,
and restored the normal installed agent under the scheduler.

## Channel cleanup soak (2026-10-21)

The preceding 0.3.3 source plus the vendored channel cleanup patch was tested
against a sacrificial SSH server using isolated profiles and listeners:

- Client-initiated close churn previously retained about 2.6–4 KB per stream:
  private bytes grew from 3.0 to 19.4 MB over 4,276 connections. The patched
  build stayed near 3.0–3.5 MB over 3,373 connections. Clean and server-initiated
  closes did not show the leak; reconnect released the retained memory.
- A 90-minute mixed-fault run completed 97 cycles with no unhealthy final
  states. The same agent PID survived throughout; private bytes plateaued near
  4.2 MB and handles remained stable. Network-change recovery did occur, so
  this does not mean uninterrupted carrier sessions throughout the soak.
- Target resets, server-side TCP rejection, half-open streams, a real Windows
  address-change notification and 70 simultaneous clients against the
  64-stream limit recovered without wedging the agent. The post-patch live
  suite also passed; physical sleep and Modern Standby were not repeated.

The source patch, original archive checksum and unpublished upstream issue
draft are retained under `agent/vendor/russh/`. Raw machine-specific evidence
remains in ignored `build/`. This is a 90-minute soak, not a 24-hour claim.

## Previous release — 0.3.3

## Release checks (2026-10-20)

- Twenty-one Rust tests, C++ core checks, native XAML startup, formatting,
  Clippy and dependency audit passed locally. Version checks cover regeneration,
  unchanged dependency pins, invalid versions and stale-package rejection.
- All twelve isolated live transport/fault checks passed with the 0.3.3 agent,
  including repeated failed optional probes, forced loss, silent blackholes,
  injected suspend/resume, strict host-key rejection and graceful cleanup.
  Supervisor crash recovery and corrupt-storage checks also passed.
- The x64 per-user installer upgraded the normal installation to 0.3.3 without
  elevation or reboot. Profiles stayed byte-identical and login startup stayed
  enabled. The installed scheduled task independently runs the agent; nested
  logins through both Prey and Brute returned the PC hostname with zero retries.
- The ARM64 CI artifact contains native ARM64 app, agent and runtime bootstrap.
  Both architectures' installer manifests passed local WinGet validation.
- Physical sleep and a 24-hour soak were not repeated for this patch.

## Post-release transport hardening (2026-10-20)

- Twenty-one Rust tests, Clippy, formatting and the C++ core check passed.
- Twelve isolated live checks passed. An always-failing optional command ran
  across two probe ticks without reconnecting SSH or losing its listener;
  identical warnings were bounded. Actual TCP loss and silent blackholes still
  triggered transport recovery. Supervisor/storage fault checks also passed.
- Two authenticated Windows SSH sessions each delivered 180 consecutive
  heartbeats over approximately three minutes while successful periodic login
  checks continued. Both completed with exit code zero and no tunnel retries.
  A successful short login also produced Windows socket reset 10054 on close;
  that reset alone does not establish a transport failure.
- Physical sleep and a 24-hour soak were not repeated in this review.

## Previous release — 0.3.2

Verified locally on 2026-10-20:

- Twenty Rust tests, the C++ core check, formatting and Clippy passed.
- Eleven isolated live checks passed, including ten deliberately reset client
  connections without an SSH reconnect, forced TCP loss, silent blackholing,
  injected suspend/resume, host-key rejection and listener cleanup.
- Regression coverage verifies that cancellation and recovery interrupt pending
  channel-open waits immediately. TCP resets retain their original error and
  endpoint details as informational stream events; other transfer errors remain
  warnings and neither stream outcome changes the tunnel state.
- The installed updated agent reached FOXE through both Prey and Brute, with
  zero transport retries after startup. Physical sleep was not repeated.
- The 0.3.2 x64 per-user installer upgraded the normal installation without a
  reboot, preserving profiles and the startup preference. The installed app
  and Windows installed-apps record both report 0.3.2.
- Native ARM64 CI passed build, unit checks, XAML startup, lint, supervisor/storage
  recovery, dependency audit and installer packaging. Both ARM64 executables and
  the shared-runtime bootstrap have native ARM64 PE headers. The complete x64
  and ARM64 WinGet manifest set passed local validation.

## Previous release — 0.3.1

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
- Native XAML startup validation loads the main screen, tray panel, editor and
  native controls without a window or agent connection. It catches the
  IconSourceElement resource-binding regression; icons use built-in Windows
  FontIcon glyphs without custom paths. This check passed locally on x64.
- The Windows ARM64 CI runner passed the release build, all 19 unit checks,
  native XAML startup, lint, formatting, supervisor/storage integration, advisory
  scan and installer packaging. App, agent and bootstrap PE headers identify
  ARM64. Physical ARM hardware UI inspection remains untested.
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
