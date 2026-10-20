# Architecture

`Connecty.exe` is a disposable C++20 / WinUI 3 frontend. `Connecty.Agent.exe`
owns connections, the tray icon, logon registration and saved configuration.
Only the agent opens SSH transports. The same Rust executable has a small
`--watchdog` role that launches `--agent` and waits on its process handle.
Abnormal exits restart with bounded backoff; exit code zero ends the launcher.
There is no watchdog status poll. Windows Task Scheduler starts this wrapper at
logon. This avoids relying on Task Scheduler's failure-retry behavior, which did
not restart forced failures on the test host. The agent separately launches the
adjacent UI on tray interaction. No component invokes an SSH executable.

| Module | Responsibility |
| --- | --- |
| `model` | Validated profiles, structured states, bounded activity |
| `config` | Compatible storage and a limited SSH alias importer |
| `ssh` | Host trust, key authentication, forwarding, probes and recovery |
| `manager` | Task ownership, configuration transactions, commands |
| `state` | Generation-guarded publication and watch notifications |
| `ipc` | Bounded framed messages and authenticated local clients |
| `platform` | SID/DACL checks, native power/network events, tray |
| `startup` | Limited per-user Task Scheduler logon registration |
| `supervisor` | Native process wait and crash-only restart backoff |

## Lifecycle

`stopped → connecting → forwarding → verified` (with a successful probe).
Transient errors enter `retrying`, then `connecting`. Windows suspend enters
`suspended`; resume or address changes interrupt backoff and rebuild transports.
Permanent trust, key, configuration and authentication failures enter `error`.
Disconnect cancels the worker, joins streams, cancels the remote listener and
disconnects SSH with bounded cleanup deadlines.

Each worker owns its transport and stream tasks. A new worker generation cannot
be overwritten by delayed updates from an old one. Changing/deleting a profile
restarts/stops only that profile; unchanged profiles continue. Disconnect does
not change saved auto-connect preferences for the next agent launch.

Probes run independently: a reverse-route check cannot block the loop that
accepts its incoming channels. Network I/O, retry timers, pipe events and native
recovery events are awaited. Keepalives and optional probes are the only periodic
connection work. Key decoding, known_hosts reads and configuration writes run on
Tokio's blocking pool, outside the reactor.

Storage uses native known-folder lookup and resolves the physical directory.
The logon action records it in `--profile-dir`; this retains the same profile
location across Windows package redirection contexts. A disabled logon task
keeps this locator without executing. `--data-dir` is instead an isolated test
directory with its own pipe name. Profile read errors are reported; only an
actual missing file is treated as an empty configuration.

## Pipe protocol v1

Default: `\\.\pipe\Connecty.Agent.<user SID>.v1`. `--data-dir` creates an isolated
hashed pipe name for tests. A first-instance pipe is owned before any connection
starts. Clients receive an immediate snapshot, then updates only on state changes.

Frames: 4-byte little-endian byte count, followed by UTF-8 JSON. Maximum 1 MiB.
After the first byte, frame assembly has a five-second deadline. Writes have a
five-second deadline. Limits: eight clients, 32 queued commands, four queued
frames/client. One reader owns frame assembly so a snapshot cannot cancel it.

```json
{"version":1,"requestId":42,"op":"connect","id":"connection-id"}
```

Operations: `snapshot`, `subscribe`, `configure` (`profiles` array), `connect`,
`disconnect`, `reconnect`, `disconnect_all`, `reconnect_all`, `startup` (`enabled`),
`shutdown`. `connect` may include an ephemeral `passphrase`. Responses:

```json
{"event":"reply","requestId":42,"ok":true,"error":null,"snapshot":{}}
```

Push events: `{"event":"snapshot","snapshot":{...}}`. Snapshot fields include
version, agent PID, startup preference, profiles, connection state map and an
optional configuration error. Errors have `code`, `message`, `retryable`.
Shutdown is acknowledged and flushed before cancellation. `--ctl '<JSON>'` uses
the same pipe checks. Never put actual passphrases in command-line arguments.

## Security boundary

Pipes reject remote clients. The DACL grants only the current user and SYSTEM.
The server validates the client process token SID; clients validate the server
SID and exact executable path. Identification quality of service prevents
server impersonation of clients. This protects against other users and pipe
collisions. It cannot protect an account from malicious code already running
as that account or replacement of binaries in a user-writable directory.

Host keys are pinned before authentication. Certificates are rejected; revoked
keys override positive records. No TOFU or host-key bypass. Secrets and probe
output never appear in activity/snapshots. Private keys remain only in active
workers. Passphrases are never persisted; input and unlock buffers are zeroized.

The agent opens no inbound administration TCP listener and never changes
firewall or SSH server settings. Probes are explicit user-supplied remote shell
commands. Local listeners are loopback-only. Remote sshd policy controls reverse
forwarding and binding (`GatewayPorts`); its administrator must preserve the
requested loopback behavior.

## Installation lifecycle

The short-lived native `setup` module runs before the Tokio transport reactor.
It captures the existing profile folder/startup preference, closes the matching
UI, requests graceful agent shutdown through its authenticated client, and
waits for process exit. Setup then rebinds the per-user task to the installed
agent and restores operation. Rollback can restart the previous installation.
Uninstall stops only its own agent/task and leaves user profiles and keys intact.
A bounded HKCU profile-folder locator preserves access after uninstall/reinstall,
including folders resolved through Windows package file redirection.

The Inno Setup package uses clean allow-listed staging. It bootstraps the shared
Microsoft UI runtime only if the native runtime probe fails, with a pinned
download hash. No UI/browser/.NET runtime is embedded in the application.
