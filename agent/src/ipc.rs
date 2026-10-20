use crate::{
    manager::{Command, Request},
    model::{AgentError, MAX_FRAME},
    state::Store,
};
use serde_json::{Value, json};
use std::{io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
    sync::{Semaphore, mpsc, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let first = reader.read_u8().await?;
    timeout(Duration::from_secs(5), async {
        let mut header = [first, 0, 0, 0];
        reader.read_exact(&mut header[1..]).await?;
        let size = u32::from_le_bytes(header) as usize;
        if size == 0 || size > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid IPC frame size",
            ));
        }
        let mut buffer = vec![0; size];
        reader.read_exact(&mut buffer).await?;
        Ok(buffer)
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Partial IPC frame timed out"))?
}
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, data: &[u8]) -> io::Result<()> {
    if data.is_empty() || data.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame too large",
        ));
    }
    timeout(Duration::from_secs(5), async {
        writer.write_u32_le(data.len() as u32).await?;
        writer.write_all(data).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "IPC write timed out"))?
}
pub fn create(
    name: &str,
    first: bool,
    security: &crate::platform::Security,
) -> io::Result<NamedPipeServer> {
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .in_buffer_size(16384)
        .out_buffer_size(16384)
        .max_instances(10);
    // The owned security descriptor remains valid for the duration of CreateNamedPipeW.
    unsafe { options.create_with_security_attributes_raw(name, security.attributes() as *mut _) }
}
pub async fn serve(
    mut listener: NamedPipeServer,
    name: String,
    security: crate::platform::Security,
    store: Arc<Store>,
    commands: mpsc::Sender<Command>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let slots = Arc::new(Semaphore::new(8));
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(_) = clients.join_next(), if !clients.is_empty() => {},
            connected = listener.connect() => {
                connected?;
                let next = create(&name, false, &security)?;
                let connection = std::mem::replace(&mut listener, next);
                if !crate::platform::same_user_client(&connection) { continue; }
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let state = store.clone();
                let tx = commands.clone();
                let cancel = shutdown.clone();
                clients.spawn(async move {
                    let _permit = permit;
                    let _ = client(connection, state, tx, cancel).await;
                });
            }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    Ok(())
}
async fn client(
    pipe: NamedPipeServer,
    store: Arc<Store>,
    commands: mpsc::Sender<Command>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let (mut reader, mut writer) = tokio::io::split(pipe);
    let mut changes = store.changes.subscribe();
    let (frames, mut received) = mpsc::channel(4);
    // A dedicated reader preserves partially received frames when status events arrive.
    let _read_task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        loop {
            let frame = read_frame(&mut reader).await;
            let done = frame.is_err();
            if frames.send(frame).await.is_err() || done {
                break;
            }
        }
    }));
    write_frame(
        &mut writer,
        &serde_json::to_vec(&json!({"event":"snapshot","snapshot":store.read()}))?,
    )
    .await?;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            changed = changes.changed() => {
                if changed.is_err() { return Ok(()); }
                let event = json!({"event":"snapshot","snapshot":store.read()});
                write_frame(&mut writer, &serde_json::to_vec(&event)?).await?;
            },
            frame = received.recv() => {
                let bytes = zeroize::Zeroizing::new(frame.ok_or_else(||io::Error::other("IPC disconnected"))??);
                let (response, stop) = dispatch(&bytes, &commands, &store).await?;
                write_frame(&mut writer, &serde_json::to_vec(&response)?).await?;
                if stop {
                    shutdown.cancel();
                    return Ok(());
                }
            }
        }
    }
}
async fn dispatch(
    bytes: &[u8],
    commands: &mpsc::Sender<Command>,
    store: &Store,
) -> io::Result<(Value, bool)> {
    let parsed: Result<Request, _> = serde_json::from_slice(bytes);
    let mut is_shutdown = false;
    let (request_id, result) = match parsed {
        Ok(request) => {
            is_shutdown = request.op == "shutdown";
            let id = request.request_id;
            let (tx, rx) = oneshot::channel();
            timeout(
                Duration::from_secs(5),
                commands.send(Command { request, reply: tx }),
            )
            .await
            .map_err(|_| io::Error::other("Agent command queue full"))?
            .map_err(|_| io::Error::other("Agent shutting down"))?;
            let result = timeout(Duration::from_secs(15), rx)
                .await
                .map_err(|_| io::Error::other("Agent command timeout"))?
                .map_err(|_| io::Error::other("Agent command canceled"))?;
            (id, result)
        }
        Err(_) => (0, Err(AgentError::config("Malformed IPC request"))),
    };
    let successful = result.is_ok();
    Ok((
        json!({"event":"reply","requestId":request_id,"ok":successful,
        "error":result.err(),"snapshot":store.read()}),
        is_shutdown && successful,
    ))
}
pub async fn control(name: &str, mut command: Value) -> io::Result<Value> {
    command["version"] = json!(1);
    command["requestId"] = json!(1);
    let mut pipe = ClientOptions::new().open(name)?;
    if !crate::platform::trusted_server(&pipe) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Untrusted agent pipe server",
        ));
    }
    write_frame(&mut pipe, &serde_json::to_vec(&command)?).await?;
    loop {
        let response: Value = serde_json::from_slice(&read_frame(&mut pipe).await?)?;
        if response["event"] == "reply" && response["requestId"] == 1 {
            return Ok(response);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn private_pipe_identity_and_exclusive_ownership() {
        let name = format!(
            r"\\.\pipe\Connecty.Test.{}.{}",
            std::process::id(),
            fastrand::u64(..)
        );
        let security = crate::platform::Security::new().unwrap();
        let server = create(&name, true, &security).unwrap();
        assert!(create(&name, true, &security).is_err());
        let client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
        assert!(crate::platform::same_user_client(&server));
        assert!(crate::platform::trusted_server(&client));
    }
    #[tokio::test]
    async fn bounded_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let write = tokio::spawn(async move {
            write_frame(&mut a, b"hello").await.unwrap();
        });
        assert_eq!(read_frame(&mut b).await.unwrap(), b"hello");
        write.await.unwrap();
    }
    #[tokio::test]
    async fn reject_oversize_before_allocation() {
        let (mut a, mut b) = tokio::io::duplex(8);
        a.write_u32_le((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
    }
    #[tokio::test(start_paused = true)]
    async fn partial_frame_deadline() {
        let (mut a, mut b) = tokio::io::duplex(8);
        a.write_u8(10).await.unwrap();
        let read = tokio::spawn(async move { read_frame(&mut b).await });
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(read.await.unwrap().is_err());
    }
}
