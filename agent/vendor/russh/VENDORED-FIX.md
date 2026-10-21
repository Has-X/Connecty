# Vendored russh 0.64.1 with a channel-reaping fix

This directory vendors `russh 0.64.1` (the exact crates.io source, Apache-2.0)
with one local fix. It is compiled via `[patch.crates-io]` in `agent/Cargo.toml`.

## The local fix

`Session::close` (`src/client/session.rs`) reaped only `Encrypted.channels`
and never removed the entry from the `Session.channels` side table
(`HashMap<ChannelId, ChannelRef>`). For every client-initiated channel close
(for example a forwarded stream whose TCP peer reset), the `ChannelRef` entry
and its buffered message channel stayed in memory until the whole SSH session
was recycled. Server-initiated closes did not leak because the
`CHANNEL_CLOSE` handler prunes both maps. Effect: linear per-connection
memory growth (~2.6-4 KB) on long-lived tunnels that carry short-lived
connections, which the event-driven recovery contract requires to survive
for days.

Change: `Session::close` additionally removes the id from `Session.channels`.
The server's later `CHANNEL_CLOSE` reply for such channels is already ignored
(`is_established_channel` is false), so no double-free or resend occurs.

## Evidence (measured on the full agent and a minimal russh client)

Reset-churn soak against a sacrificial server, agent private bytes:
- before: ~2.6 MB baseline growing linearly ~2.6-4 KB per reset connection
  (2.6 -> 8.0 MB over 1200 resets), retained after idle, freed only by
  `reconnect_all`.
- after: baseline ~2.8 MB, flat across the same churn (3.08 -> 3.08 -> 3.25 MB).
Traced in an instrumented copy: 1200/1200 client-initiated closes previously
left the side-table entry in place; with the fix all 1200 are reaped.

## Provenance

- Upstream: https://github.com/Eugeny/russh (v0.64.1, Apache-2.0)
- crates.io archive SHA-256: `ba61e87b9ec9a39a59a6bbed4c0b8ff7fb07073405b24a767a1f2c8560ac8f58`.
- Upstream revision recorded by the crate: `e5d80a07dc554480b6ea3b2e14adfc190407b627`.
- Connecty modification: 2026-10-21. `LICENSE` reproduces Apache-2.0;
  `UPSTREAM-ISSUE.md` is an unpublished issue draft. A source comparison against
  the local registry copy found no other code changes (ignoring line endings).
- Local diff vs crates.io 0.64.1: `Session::close` in `src/client/session.rs`
  only; rebuild with `cargo build --locked` after `cargo update -p russh`
  is NOT needed because the version is unchanged (0.64.1).
- When a russh release contains the fix, drop this vendored copy and the
  `[patch.crates-io]` section, and re-pin `russh = "=<new>"` in Cargo.toml.
