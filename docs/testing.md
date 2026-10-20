# Verification

`scripts/build.ps1 -Test -Package` runs Rust and C++ unit checks for profile
migration/validation, atomic replacement, alias parsing, host-key rejection,
log bounds, framing/deadlines, backoff and frontend helpers. Run Clippy with
warnings denied before release.

Run `scripts/test-version.ps1 -Architecture x64` after building (use `arm64` on
that architecture). It checks future-version parsing, generated Windows metadata,
automatic CMake regeneration, lockfile preservation and stale-package rejection.

The separate process-recovery test uses a fresh empty data directory and kills
only its authenticated, isolated child agent:

```powershell
python scripts/test-supervisor.py --agent build/native/Connecty.Agent.exe --data build/supervisor-unique
```

It verifies an abnormal child exit restarts, a duplicate launcher is harmless,
and an acknowledged shutdown ends both processes. The small launcher uses a
native process wait while healthy, with a timer only after a child failure.
It also proves corrupt saved configuration is reported and rejected writes
cannot replace it. CI runs this isolated test without requiring an SSH server.

## Real-server fault suite

The opt-in Python standard-library harness uses a transparent TCP fault relay.
Every SSH connection is made by the embedded agent. `ssh-keygen -F` only reads
an already trusted public key to map that key to the relay; it never establishes
trust or loads private keys.

```powershell
python scripts/test-live.py --agent build/native/Connecty.Agent.exe `
  --data build/live-unique --remote-host your-server.example `
  --user your-user --key C:\path\to\private-key --blackhole
```

Use a fresh data directory, an unencrypted test identity, a pinned Ed25519 host
key, and Linux SSH server with Python 3. The PC return-path assertion expects a
Windows SSH service on local port 22. Default ports: reverse 22031, local 22631;
override them if owned. Changed-key testing needs a second existing public
Ed25519 known_hosts entry. User profiles and known_hosts are never modified.

Checks: simultaneous forwarding directions, verified Windows SSH banner through
the reverse route, agent-only operation, repeated client TCP resets without an
SSH reconnect, forced TCP loss and independent recovery,
injected power recovery, unknown/changed host keys, silent blackholing, listener
cleanup and acknowledged shutdown. Results stay in the ignored build directory.
Expected forced relay resets are not application failures.

`stream_reset` records an individual forwarded connection reset, with the peer,
destination and original OS error. It does not change tunnel state. Other transfer
errors remain warnings. Unit tests also check that recovery and cancellation can
interrupt a pending channel-open operation before its timeout expires.

Power-event injection requires `--test-mode`; normal agents reject it. It tests
dispatch and cleanup, **not physical Windows sleep**.

## Interactive and host checks

- Edit/save/select multiple profiles; check state isolation and compact activity.
- Right-click tray; check WinUI panel bounds, keyboard controls, light/dark and DPI.
- Close UI; verify its process exits while the same agent PID and both routes
  remain. Reopen and attach to that agent.
- Enable startup; inspect the task's interactive-token/limited principal, logon
  trigger, unlimited execution, battery and crash restart settings. Task
  registration alone does not prove a real logout/login.
- For physical sleep, verify an automatic wake timer first, enter sleep, then
  collect Windows power-event evidence and check both routes after resume.
  Record the power model; S3 and Modern Standby differ.
- Measure agent CPU time and working/private memory with UI closed during a
  quiet interval. Keepalive traffic is expected; periodic status polling is not.

A short smoke test does not establish a 24/7 soak. Keep long-duration,
different-network and Modern Standby tests explicit in release validation.
