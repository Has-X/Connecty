use crate::{
    config::{self, Resolved},
    model::{AgentError, Direction, MAX_STREAMS, Profile},
    state::Publisher,
};
use russh::{
    Channel, ChannelMsg, ChannelOpenFailure, client,
    keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate},
};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    io,
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    task::JoinSet,
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[derive(Clone, Debug, PartialEq)]
pub enum Recovery {
    Ready,
    Network,
    Suspend,
    Resume,
}
#[derive(Debug, thiserror::Error)]
pub enum Failure {
    #[error(transparent)]
    Ssh(#[from] russh::Error),
    #[error(transparent)]
    Policy(#[from] AgentError),
}
impl Failure {
    fn agent(self) -> AgentError {
        match self {
            Self::Policy(e) => e,
            Self::Ssh(e) => AgentError::network(format!("SSH: {e}")),
        }
    }
}
pub fn verify_key(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), AgentError> {
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let text = std::fs::read_to_string(path).map_err(|_| {
        AgentError::new(
            "host_key_unknown",
            format!("No trusted host key for {host}:{port}. Fingerprint {fingerprint}"),
            false,
        )
    })?;
    // Revocations override positive entries. Never treat certificates as plain keys.
    for line in text.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.first() == Some(&"@revoked")
            && parts.len() >= 4
            && PublicKey::from_openssh(&format!("{} {}", parts[2], parts[3]))
                .ok()
                .as_ref()
                == Some(key)
        {
            return Err(AgentError::new(
                "host_key_revoked",
                "The server host key is revoked",
                false,
            ));
        }
    }
    match russh::keys::check_known_hosts_path(host, port, key, path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AgentError::new(
            "host_key_unknown",
            format!(
                "Unknown host key for {host}:{port}. Verify {fingerprint} before adding it to known_hosts"
            ),
            false,
        )),
        Err(russh::keys::Error::KeyChanged { .. }) => Err(AgentError::new(
            "host_key_changed",
            format!("HOST KEY CHANGED for {host}:{port}. Received {fingerprint}"),
            false,
        )),
        Err(e) => Err(AgentError::new(
            "host_key_invalid",
            format!("Cannot verify host key: {e}"),
            false,
        )),
    }
}
struct Incoming {
    channel: Channel<client::Msg>,
    reply: client::ChannelOpenHandle,
    origin: String,
}
struct Handler {
    endpoint: Resolved,
    port: u16,
    bind: String,
    incoming: mpsc::Sender<Incoming>,
}
impl client::Handler for Handler {
    type Error = Failure;
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Failure> {
        if key.certificate().is_some() {
            return Err(AgentError::new(
                "host_certificate_unsupported",
                "Host certificates are not supported; use a pinned raw host key",
                false,
            )
            .into());
        }
        let endpoint = self.endpoint.clone();
        let key = key.public_key();
        tokio::task::spawn_blocking(move || {
            verify_key(
                &endpoint.known_hosts,
                &endpoint.key_host,
                endpoint.port,
                &key,
            )
        })
        .await
        .map_err(|_| AgentError::network("Host-key verification worker stopped"))??;
        Ok(true)
    }
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        address: &str,
        port: u32,
        origin: &str,
        origin_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Failure> {
        if port != u32::from(self.port) || address != self.bind {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        // A full bounded queue drops the reply handle, rejecting the channel.
        let _ = self.incoming.try_send(Incoming {
            channel,
            reply,
            origin: format!("{origin}:{origin_port}"),
        });
        Ok(())
    }
}
fn network(e: impl std::fmt::Display) -> AgentError {
    AgentError::network(e.to_string())
}
// Race the entire operation, including nested channel-open waits, against recovery.
// Dropping work first releases its borrowed handle before session cleanup runs.
async fn interruptible<T>(
    cancel: &CancellationToken,
    recovery: &mut watch::Receiver<Recovery>,
    work: impl std::future::Future<Output = Result<T, AgentError>>,
) -> Result<Option<T>, AgentError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(None),
        _ = async {
            loop {
                if recovery.changed().await.is_err()
                    || matches!(*recovery.borrow_and_update(), Recovery::Suspend | Recovery::Resume)
                {
                    break;
                }
                // Interface notifications also describe unrelated adapters.
                // Preserve an established SSH transport; keepalives detect an
                // actual outage. Connecting/backoff still react to these hints.
            }
        } => Err(network("Power state changed")),
        result = work => result.map(Some),
    }
}

fn log_stream_result(
    publisher: &Publisher,
    origin: &str,
    target: &str,
    port: u16,
    result: io::Result<(u64, u64)>,
) {
    match result {
        Ok((received, sent)) => publisher.log(
            "info",
            "stream_closed",
            &format!("{origin} disconnected; {received} bytes received, {sent} sent"),
        ),
        // A reset terminates this stream, not the persistent SSH transport. Keep
        // the cause visible without presenting a client disconnect as tunnel failure.
        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => publisher.log(
            "info",
            "stream_reset",
            &format!("Forwarded connection reset ({origin} to {target}:{port}): {e}"),
        ),
        Err(e) => publisher.log(
            "warn",
            "stream_error",
            &format!("Forwarded connection failed ({origin} to {target}:{port}): {e}"),
        ),
    }
}
pub fn backoff(attempt: u32, random: f64) -> Duration {
    let base = (1u64 << attempt.min(6)).min(60) as f64;
    Duration::from_secs_f64((base * (0.75 + random.clamp(0.0, 1.0) * 0.5)).min(75.0))
}

async fn connect(
    p: &Profile,
    endpoint: &Resolved,
    key: Arc<PrivateKey>,
    incoming: mpsc::Sender<Incoming>,
) -> Result<client::Handle<Handler>, AgentError> {
    let cfg = client::Config {
        keepalive_interval: Some(Duration::from_secs(20)),
        keepalive_max: 3,
        inactivity_timeout: None,
        window_size: 256 * 1024,
        channel_buffer_size: 8,
        nodelay: true,
        ..Default::default()
    };
    let handler = Handler {
        endpoint: endpoint.clone(),
        port: p.remote_port,
        bind: p.bind_address.clone(),
        incoming,
    };
    let mut session = timeout(
        Duration::from_secs(15),
        client::connect(
            Arc::new(cfg),
            (endpoint.host.as_str(), endpoint.port),
            handler,
        ),
    )
    .await
    .map_err(|_| network("SSH handshake timed out"))?
    .map_err(Failure::agent)?;
    let result = timeout(Duration::from_secs(15), async {
        session
            .authenticate_publickey(&endpoint.user, PrivateKeyWithHashAlg::new(key, None))
            .await
            .map_err(network)
    })
    .await
    .map_err(|_| network("Key authentication timed out"))??;
    if !result.success() {
        let _ = session
            .disconnect(
                russh::Disconnect::ByApplication,
                "Authentication rejected",
                "en",
            )
            .await;
        return Err(AgentError::new(
            "authentication",
            "The server rejected this SSH key",
            false,
        ));
    }
    Ok(session)
}
#[derive(Default)]
struct VerificationStatus {
    error: Option<AgentError>,
}
impl VerificationStatus {
    fn update(&mut self, publisher: &Publisher, check: Result<(), AgentError>) {
        match check {
            Ok(()) => {
                if self.error.take().is_some() {
                    publisher.log(
                        "info",
                        "verification_recovered",
                        "Return-path check recovered",
                    );
                }
                publisher.status("verified", "Return path checked", None);
            }
            Err(error) => {
                if self.error.as_ref().is_none_or(|previous| {
                    previous.code != error.code || previous.message != error.message
                }) {
                    publisher.log("warn", &error.code, &error.message);
                }
                publisher.status(
                    "forwarding",
                    "Return-path check failed",
                    Some(error.clone()),
                );
                self.error = Some(error);
            }
        }
    }
}

async fn probe(mut channel: Channel<client::Msg>, command: String) -> Result<(), AgentError> {
    timeout(Duration::from_secs(12), async {
        channel.exec(true, command).await.map_err(network)?;
        let mut status = None;
        let mut bytes = 0usize;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    bytes += data.len();
                    if bytes > 8192 {
                        let _ = channel.close().await;
                        return Err(AgentError::new(
                            "verification_output",
                            "Probe output exceeded 8 KiB",
                            true,
                        ));
                    }
                }
                _ => {}
            }
        }
        match status {
            Some(0) => Ok(()),
            Some(code) => Err(AgentError::new(
                "verification_failed",
                format!("Probe returned exit code {code}"),
                true,
            )),
            None => Err(AgentError::new(
                "verification_failed",
                "Probe ended without an exit status",
                true,
            )),
        }
    })
    .await
    .map_err(|_| AgentError::new("verification_timeout", "Probe timed out", true))?
}
async fn session(
    p: &Profile,
    endpoint: &Resolved,
    key: Arc<PrivateKey>,
    publisher: &Publisher,
    cancel: &CancellationToken,
    recovery: &mut watch::Receiver<Recovery>,
) -> Result<(), AgentError> {
    let (tx, mut incoming) = mpsc::channel(16);
    publisher.status("connecting", "Connecting securely", None);
    let mut handle = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        _ = recovery.changed() => return Err(network("Network state changed")),
        result = connect(p, endpoint, key, tx) => result?,
    };
    let listener = if p.direction == Direction::Reverse {
        let requested = timeout(
            Duration::from_secs(10),
            handle.tcpip_forward(&p.bind_address, u32::from(p.remote_port)),
        )
        .await
        .map_err(|_| network("Remote listener request timed out"))?;
        if let Err(error) = requested {
            if matches!(error, russh::Error::RequestDenied) {
                // Touching only our reserved listener nudges a stale TCP session to
                // discover its dead peer. A healthy owner is never terminated.
                if let Ok(Ok(channel)) = timeout(
                    Duration::from_secs(2),
                    handle.channel_open_direct_tcpip(
                        &p.bind_address,
                        u32::from(p.remote_port),
                        "127.0.0.1",
                        0,
                    ),
                )
                .await
                {
                    let _ = channel.close().await;
                }
            }
            return Err(AgentError::new(
                "forward_rejected",
                format!("Remote listening port is unavailable: {error}"),
                true,
            ));
        }
        None
    } else {
        Some(
            TcpListener::bind((p.bind_address.as_str(), p.remote_port))
                .await
                .map_err(|e| {
                    AgentError::new(
                        "bind_failed",
                        format!("Cannot open local listener: {e}"),
                        false,
                    )
                })?,
        )
    };
    publisher.status("forwarding", "Tunnel is ready", None);
    publisher.log(
        "info",
        "connected",
        &format!(
            "Authenticated {}@{}:{}; {}:{} forwards to {}:{}",
            endpoint.user,
            endpoint.host,
            endpoint.port,
            p.bind_address,
            p.remote_port,
            p.target,
            p.target_port
        ),
    );
    let mut streams = JoinSet::new();
    let mut verification = JoinSet::new();
    let mut verify = tokio::time::interval(Duration::from_secs(60));
    verify.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut verification_status = VerificationStatus::default();
    let result = interruptible::<()>(cancel, recovery, async {
      loop {
        tokio::select! {
            biased;
            result = &mut handle => break Err(match result {
                Ok(()) => network("SSH connection closed"),
                Err(e) => e.agent(),
            }),
            Some(_) = streams.join_next(), if !streams.is_empty() => {},
            Some(request) = incoming.recv(), if p.direction == Direction::Reverse => {
                if streams.len() >= MAX_STREAMS { drop(request); continue; }
                let target = p.target.clone();
                let port = p.target_port;
                let log = publisher.clone();
                streams.spawn(async move {
                    match timeout(Duration::from_secs(10), TcpStream::connect((target.as_str(), port))).await {
                        Ok(Ok(mut socket)) => {
                            request.reply.accept().await;
                            log.log("info", "stream_open", &format!("{} connected to {target}:{port} (reverse)", request.origin));
                            let mut channel = request.channel.into_stream();
                            let result = io::copy_bidirectional(&mut channel, &mut socket).await;
                            log_stream_result(&log, &request.origin, &target, port, result);
                        }
                        _ => {
                            log.log("warn", "stream_rejected", &format!("{} could not reach {target}:{port}", request.origin));
                            request.reply.reject(ChannelOpenFailure::ConnectFailed).await;
                        },
                    }
                });
            },
            accepted = async {
                match &listener { Some(l) => l.accept().await, None => std::future::pending().await }
            }, if listener.is_some() && streams.len() < MAX_STREAMS => {
                let (mut socket, origin) = accepted.map_err(network)?;
                let opened = timeout(Duration::from_secs(10), handle.channel_open_direct_tcpip(
                        &p.target, u32::from(p.target_port), origin.ip().to_string(), u32::from(origin.port())
                    )).await;
                let channel = match opened {
                    Ok(Ok(channel)) => channel,
                    _ => {
                        publisher.log("warn", "stream_rejected", "The destination refused a forwarded connection");
                        continue;
                    }
                };
                publisher.log("info", "stream_open", &format!("{origin} connected to {}:{} (local)", p.target, p.target_port));
                let log = publisher.clone();
                let target = p.target.clone();
                let port = p.target_port;
                streams.spawn(async move {
                    let mut channel = channel.into_stream();
                    let result = io::copy_bidirectional(&mut channel, &mut socket).await;
                    log_stream_result(&log, &origin.to_string(), &target, port, result);
                });
            },
            _ = verify.tick(), if !p.probe.is_empty() && verification.is_empty() => {
                let opened = timeout(Duration::from_secs(5), handle.channel_open_session()).await;
                let command = p.probe.clone();
                verification.spawn(async move {
                    match opened {
                        Ok(Ok(channel)) => probe(channel, command).await,
                        Ok(Err(e)) => Err(network(e)),
                        Err(_) => Err(network("Verification channel timed out")),
                    }
                });
            },
            Some(check) = verification.join_next(), if !verification.is_empty() => {
                let check = check.unwrap_or_else(|_| Err(network("Verification worker stopped")));
                // An optional command can fail independently of the SSH transport
                // (destination authentication, permissions or a slow service).
                // Keep active streams; keepalives detect a genuinely stale session.
                verification_status.update(publisher, check);
            }
        }
      }
    }).await.map(|_| ());
    // Cancellation owns every forwarded socket; no foreign session is killed.
    streams.abort_all();
    while streams.join_next().await.is_some() {}
    verification.abort_all();
    while verification.join_next().await.is_some() {}
    if p.direction == Direction::Reverse && !handle.is_closed() {
        let _ = timeout(
            Duration::from_secs(1),
            handle.cancel_tcpip_forward(&p.bind_address, u32::from(p.remote_port)),
        )
        .await;
    }
    let _ = timeout(
        Duration::from_secs(1),
        handle.disconnect(
            russh::Disconnect::ByApplication,
            "Connecty disconnect",
            "en",
        ),
    )
    .await;
    result
}
pub async fn run(
    p: Profile,
    password: Option<Zeroizing<String>>,
    publisher: Publisher,
    cancel: CancellationToken,
    mut recovery: watch::Receiver<Recovery>,
) {
    let loaded = async {
        let cfg = p.clone();
        let endpoint = tokio::task::spawn_blocking(move || config::resolve(&cfg))
            .await
            .map_err(|_| AgentError::config("Configuration worker stopped"))??;
        let path = endpoint.identity.clone();
        let key = tokio::task::spawn_blocking(move || {
            let text = std::fs::read_to_string(path).map(Zeroizing::new).map_err(|_| {
                AgentError::new("key_file", "Cannot read the private key file", false)
            })?;
            let encrypted = PrivateKey::from_openssh(text.as_str()).map(|key| key.is_encrypted()).unwrap_or(false);
            let key = russh::keys::decode_secret_key(&text, password.as_deref().map(|s| s.as_str())).map_err(
                |error| {
                    if !encrypted && !matches!(error, russh::keys::Error::KeyIsEncrypted) {
                        return AgentError::new("key_format", "Unsupported or invalid private key. Use Ed25519 or ECDSA in OpenSSH format", false);
                    }
                    AgentError::new(
                        "key_locked",
                        "Cannot unlock the private key. Check its path and passphrase",
                        false,
                    )
                },
            )?;
            if !matches!(key.algorithm(), russh::keys::Algorithm::Ed25519 | russh::keys::Algorithm::Ecdsa { .. }) {
                return Err(AgentError::new("key_algorithm", "Use an Ed25519 or ECDSA key. RSA is disabled", false));
            }
            Ok(key)
        })
        .await
        .map_err(|_| AgentError::config("Key worker stopped"))??;
        Ok::<_, AgentError>((endpoint, Arc::new(key)))
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => return,
        result = loaded => result,
    };
    let (endpoint, key) = match result {
        Ok(v) => v,
        Err(e) => {
            publisher.log("error", &e.code, &e.message);
            publisher.status("error", &e.message, Some(e.clone()));
            return;
        }
    };
    let mut attempt = 0;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        // A newly cloned receiver can contain old hints from before this
        // connection existed. Consume that state before opening fresh sockets.
        while *recovery.borrow_and_update() == Recovery::Suspend {
            publisher.status("suspended", "Waiting for Windows to resume", None);
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = recovery.changed() => {},
            }
        }
        let started = Instant::now();
        match session(
            &p,
            &endpoint,
            key.clone(),
            &publisher,
            &cancel,
            &mut recovery,
        )
        .await
        {
            Ok(()) => break,
            Err(e) => {
                publisher.log(
                    if e.retryable { "warn" } else { "error" },
                    &e.code,
                    &e.message,
                );
                if !e.retryable || !p.retry {
                    publisher.status("error", &e.message, Some(e.clone()));
                    return;
                }
                if started.elapsed() > Duration::from_secs(90) {
                    attempt = 0;
                }
                let delay = backoff(attempt, fastrand::f64());
                attempt = attempt.saturating_add(1);
                publisher.retry();
                publisher.status(
                    "retrying",
                    &format!("Reconnecting in {}s", delay.as_secs().max(1)),
                    Some(e),
                );
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = sleep(delay) => {},
                    _ = recovery.changed() => {},
                }
            }
        }
    }
    publisher.status("stopped", "Disconnected", None);
    publisher.log("info", "disconnected", "Disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn network_hint_preserves_pending_work_until_power_recovery() {
        let cancel = CancellationToken::new();
        let (events, mut recovery) = watch::channel(Recovery::Ready);
        let (entered, started) = tokio::sync::oneshot::channel();
        let mut task = tokio::spawn(async move {
            interruptible::<()>(&cancel, &mut recovery, async {
                let _ = entered.send(());
                std::future::pending().await
            })
            .await
        });
        started.await.unwrap();
        events.send(Recovery::Network).unwrap();
        assert!(timeout(Duration::from_millis(50), &mut task).await.is_err());
        events.send(Recovery::Suspend).unwrap();
        assert!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
    #[test]
    fn repeated_probe_failures_keep_transport_state_and_recover_without_retries() {
        let mut snapshot = crate::model::Snapshot::default();
        let mut connection = crate::model::ConnectionState::default();
        connection.set("verified", "Return path checked", None);
        connection.verified_at = Some(crate::model::now());
        snapshot.connections.insert("test".into(), connection);
        let store = crate::state::Store::new(snapshot);
        let publisher = Publisher {
            store: store.clone(),
            id: "test".into(),
            generation: 0,
        };
        let since = store.read().connections["test"].since;
        let mut status = VerificationStatus::default();
        for _ in 0..10 {
            status.update(
                &publisher,
                Err(AgentError::new(
                    "verification_failed",
                    "Probe returned exit code 1",
                    true,
                )),
            );
        }
        let snapshot = store.read();
        let failed = &snapshot.connections["test"];
        assert_eq!(failed.state, "forwarding");
        assert_eq!(failed.since, since);
        assert_eq!(failed.retries, 0);
        assert!(failed.verified_at.is_none());
        assert_eq!(failed.error.as_ref().unwrap().code, "verification_failed");
        assert_eq!(failed.logs.len(), 1);
        status.update(
            &publisher,
            Err(AgentError::new(
                "verification_timeout",
                "Probe timed out",
                true,
            )),
        );
        status.update(&publisher, Ok(()));
        status.update(&publisher, Ok(()));
        let snapshot = store.read();
        let recovered = &snapshot.connections["test"];
        assert_eq!(recovered.state, "verified");
        assert_eq!(recovered.since, since);
        assert_eq!(recovered.retries, 0);
        assert!(recovered.error.is_none() && recovered.verified_at.is_some());
        assert_eq!(recovered.logs.len(), 3);
        assert_eq!(recovered.logs[2].code, "verification_recovered");
    }

    #[test]
    fn stream_reset_is_distinct_from_transfer_failure_and_keeps_tunnel_state() {
        let mut snapshot = crate::model::Snapshot::default();
        let mut connection = crate::model::ConnectionState::default();
        connection.set("verified", "Return path checked", None);
        snapshot.connections.insert("test".into(), connection);
        let store = crate::state::Store::new(snapshot);
        let publisher = Publisher {
            store: store.clone(),
            id: "test".into(),
            generation: 0,
        };
        log_stream_result(
            &publisher,
            "127.0.0.1:1234",
            "127.0.0.1",
            22,
            Err(io::Error::from_raw_os_error(10054)),
        );
        log_stream_result(
            &publisher,
            "127.0.0.1:1234",
            "127.0.0.1",
            22,
            Err(io::Error::new(io::ErrorKind::TimedOut, "transfer stalled")),
        );
        let snapshot = store.read();
        let state = &snapshot.connections["test"];
        assert_eq!(state.state, "verified");
        assert_eq!(state.retries, 0);
        assert_eq!(state.logs[0].code, "stream_reset");
        assert_eq!(state.logs[0].level, "info");
        assert!(state.logs[0].message.contains("10054"));
        assert!(state.logs[0].message.contains("127.0.0.1:22"));
        assert_eq!(state.logs[1].code, "stream_error");
        assert_eq!(state.logs[1].level, "warn");
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_interrupts_nested_channel_wait_without_its_timeout() {
        for cancel_work in [false, true] {
            let cancel = CancellationToken::new();
            let signal = cancel.clone();
            let (events, mut recovery) = watch::channel(Recovery::Ready);
            let (entered, started) = tokio::sync::oneshot::channel();
            let (released, dropped) = tokio::sync::oneshot::channel::<()>();
            let task = tokio::spawn(async move {
                interruptible(&cancel, &mut recovery, async {
                    // The guard proves cancellation drops in-flight work before cleanup.
                    let _guard = released;
                    entered.send(()).unwrap();
                    std::future::pending::<Result<(), AgentError>>().await
                })
                .await
            });
            started.await.unwrap();
            let before = Instant::now();
            if cancel_work {
                signal.cancel();
            } else {
                events.send(Recovery::Suspend).unwrap();
            }
            let result = task.await.unwrap();
            assert_eq!(Instant::now(), before);
            assert!(dropped.await.is_err());
            if cancel_work {
                assert!(matches!(result, Ok(None)));
            } else {
                assert!(result.unwrap_err().retryable);
            }
        }
    }

    #[test]
    fn jitter_bounded() {
        for n in 0..100 {
            let a = backoff(n, 0.0);
            let b = backoff(n, 1.0);
            assert!(a <= b && b <= Duration::from_secs(75));
            assert!(a >= Duration::from_millis(750));
        }
    }
    #[test]
    fn host_key_unknown_changed_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
        let other = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
        assert_eq!(
            verify_key(&path, "test", 22, key.public_key())
                .unwrap_err()
                .code,
            "host_key_unknown"
        );
        let public = key.public_key().to_openssh().unwrap();
        std::fs::write(&path, format!("test {public}\n")).unwrap();
        verify_key(&path, "test", 22, key.public_key()).unwrap();
        assert_eq!(
            verify_key(&path, "test", 22, other.public_key())
                .unwrap_err()
                .code,
            "host_key_changed"
        );
        std::fs::write(&path, format!("test {public}\n@revoked test {public}\n")).unwrap();
        assert_eq!(
            verify_key(&path, "test", 22, key.public_key())
                .unwrap_err()
                .code,
            "host_key_revoked"
        );
    }
}
