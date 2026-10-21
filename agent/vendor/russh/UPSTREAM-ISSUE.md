# Upstream issue draft: Eugeny/russh 0.64.1 — client-initiated channel close leaks the ChannelRef entry in Session.channels

## Summary

`Session::close` (src/client/session.rs) reaps only `Encrypted.channels`
(the ChannelParams side table) and never removes the matching entry from
`Session.channels: HashMap<ChannelId, ChannelRef>`. Every **client-initiated**
channel close therefore leaks one `ChannelRef` entry plus its buffered
tokio mpsc until the whole SSH session is recycled.

Server-initiated closes do not leak: the CHANNEL_CLOSE handler in
src/client/encrypted.rs removes from both maps.

## Reproduction

A minimal russh client that mirrors the standard port-forwarding pattern
(request remote forward, accept forwarded-tcpip, `copy_bidirectional` against
the local target, drop the channel when the peer resets) leaks ~2.6-4 KB of
private memory per connection whose teardown is client-initiated (i.e. the
local target socket died first). Connections closed by the server first
(clean EOF, or RST before any data) do not leak, because the server's
CHANNEL_CLOSE handler prunes the map.

Measured with a 0.64.1 debug client, 1200 banner+RST connections through the
forwarded port:

- private bytes: 2.6 MB -> 8.0 MB (linear, no plateau), retained after idle,
- released only when the SSH session is re-established.

A long-lived tunnel carrying many short-lived connections (the common
reverse-tunnel use case) grows without bound until the session is recycled.

## Why the map entry is never pruned on client-initiated close

1. Dropping the channel handle sends `ChannelMsg::Close` to the session loop.
2. `Msg::Channel(id, ChannelMsg::Close) => self.close(id)?`
   (src/client/mod.rs:1728) -> `Session::close` (src/client/session.rs:540)
   -> `Encrypted::close` (src/session.rs:287), which removes the id from
   `Encrypted.channels` only.
3. When the server later sends CHANNEL_CLOSE for that channel, the handler
   in src/client/encrypted.rs returns early via
   `if !self.common.is_established_channel(channel_num) { return Ok(()); }`
   because the id is already gone from `Encrypted.channels` — so the
   `self.channels.remove(&channel_num)` on line 522 is never reached.

## Fix (validated)

In `Session::close`, also remove the id from `self.channels`:

```rust
pub fn close(&mut self, channel: ChannelId) -> Result<(), crate::Error> {
    if let Some(ref mut enc) = self.common.encrypted {
        let result = enc.close(channel);
        self.channels.remove(&channel);
        result
    } else {
        unreachable!()
    }
}
```

With this change the same 1200-connection churn stays flat
(2.8 MB -> 3.1 MB, plateaued) and all entries are reaped.

## Environment

russh 0.64.1 (crates.io), tokio 1.53, Windows 11 client, OpenSSH 9.6 server.
