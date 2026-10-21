use crate::{
    config,
    model::*,
    ssh::{self, Recovery},
    state::{Publisher, Store},
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub version: u32,
    #[serde(default)]
    pub request_id: u64,
    pub op: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}
impl Drop for Request {
    fn drop(&mut self) {
        if let Some(passphrase) = &mut self.passphrase {
            passphrase.zeroize();
        }
    }
}
pub struct Command {
    pub request: Request,
    pub reply: oneshot::Sender<Result<(), AgentError>>,
}
struct Running {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}
pub struct Manager {
    store: Arc<Store>,
    path: PathBuf,
    running: HashMap<String, Running>,
    desired: HashSet<String>,
    generation: u64,
    recovery: watch::Receiver<Recovery>,
    readonly: bool,
    test_mode: bool,
}
impl Manager {
    pub fn new(
        store: Arc<Store>,
        path: PathBuf,
        recovery: watch::Receiver<Recovery>,
        readonly: bool,
        test_mode: bool,
    ) -> Self {
        Self {
            store,
            path,
            running: HashMap::new(),
            desired: HashSet::new(),
            generation: 0,
            recovery,
            readonly,
            test_mode,
        }
    }
    async fn start(
        &mut self,
        id: &str,
        password: Option<Zeroizing<String>>,
    ) -> Result<(), AgentError> {
        let p = self
            .store
            .read()
            .profiles
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| AgentError::config("Connection not found"))?;
        self.stop(id).await;
        self.desired.insert(id.into());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.store.update(|s| {
            let c = s.connections.entry(id.into()).or_default();
            c.generation = generation;
            c.set("connecting", "Connecting securely", None);
        });
        let publisher = Publisher {
            store: self.store.clone(),
            id: id.into(),
            generation,
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(ssh::run(
            p,
            password,
            publisher,
            cancel.clone(),
            self.recovery.clone(),
        ));
        self.running.insert(id.into(), Running { cancel, task });
        Ok(())
    }
    async fn stop(&mut self, id: &str) {
        self.stop_many(&[id.to_string()]).await;
    }
    async fn stop_many(&mut self, ids: &[String]) {
        let mut tasks = Vec::new();
        for id in ids {
            self.desired.remove(id);
            if let Some(running) = self.running.remove(id) {
                running.cancel.cancel();
                tasks.push(running.task);
            }
        }
        if timeout(Duration::from_secs(3), async {
            for t in &mut tasks {
                let _ = t.await;
            }
        })
        .await
        .is_err()
        {
            for t in &tasks {
                t.abort();
            }
            for t in tasks {
                // Earlier entries may already have been joined before timeout.
                if !t.is_finished() {
                    let _ = t.await;
                }
            }
        }
        self.store.update(|s| {
            for id in ids {
                if let Some(c) = s.connections.get_mut(id) {
                    c.set("stopped", "Disconnected", None);
                }
            }
        });
    }
    async fn all_stop(&mut self) {
        let ids: Vec<_> = self.store.read().connections.keys().cloned().collect();
        self.stop_many(&ids).await;
    }
    async fn apply(&mut self, mut r: Request) -> Result<(), AgentError> {
        if r.version != 1 {
            return Err(AgentError::new(
                "protocol_version",
                "Unsupported IPC version",
                false,
            ));
        }
        match r.op.as_str() {
            "snapshot" | "subscribe" => {}
            "connect" | "reconnect" => {
                self.start(&r.id, r.passphrase.take().map(Zeroizing::new))
                    .await?
            }
            "disconnect" => self.stop(&r.id).await,
            "disconnect_all" => self.all_stop().await,
            "reconnect_all" => {
                let ids: Vec<_> = self
                    .store
                    .read()
                    .profiles
                    .into_iter()
                    .map(|p| p.id)
                    .collect();
                self.all_stop().await;
                for id in ids {
                    self.start(&id, None).await?;
                }
            }
            "configure" => {
                if self.readonly {
                    return Err(AgentError::config(
                        "Repair saved profiles before overwriting them",
                    ));
                }
                validate_profiles(&r.profiles)?;
                let path = self.path.clone();
                let next = r.profiles.clone();
                tokio::task::spawn_blocking(move || config::save(&path, &next))
                    .await
                    .map_err(|_| AgentError::config("Save worker stopped"))??;
                let old = self.store.read().profiles;
                let wanted = self.desired.clone();
                let changed: Vec<_> = old
                    .iter()
                    .filter(|p| !r.profiles.contains(p))
                    .map(|p| p.id.clone())
                    .collect();
                self.stop_many(&changed).await;
                self.store.update(|s| {
                    s.profiles = r.profiles.clone();
                    s.connections
                        .retain(|id, _| s.profiles.iter().any(|p| &p.id == id));
                    for p in &s.profiles {
                        s.connections.entry(p.id.clone()).or_insert_with(|| {
                            let mut c = ConnectionState::default();
                            c.set("stopped", "Ready", None);
                            c
                        });
                    }
                    s.error = None;
                });
                for p in &r.profiles {
                    let is_new = !old.iter().any(|o| o.id == p.id);
                    if (is_new && p.auto_connect)
                        || (changed.contains(&p.id) && wanted.contains(&p.id))
                    {
                        self.start(&p.id, None).await?;
                    }
                }
            }
            "startup" => {
                let dir = self
                    .path
                    .parent()
                    .ok_or_else(|| AgentError::config("Invalid profile directory"))?
                    .to_path_buf();
                tokio::task::spawn_blocking(move || crate::startup::set(r.enabled, &dir))
                    .await
                    .map_err(|_| AgentError::config("Startup registration worker stopped"))?
                    .map_err(|e| AgentError::config(e.to_string()))?;
                self.store.update(|s| s.startup = r.enabled);
            }
            // IPC acknowledges and flushes this command before cancellation.
            "shutdown" => {}
            "test_recovery" if self.test_mode => {
                crate::platform::inject_recovery(if r.enabled {
                    Recovery::Resume
                } else {
                    Recovery::Suspend
                });
            }
            "test_network" if self.test_mode => crate::platform::inject_recovery(Recovery::Network),
            _ => return Err(AgentError::config("Unknown agent command")),
        }
        Ok(())
    }
    pub async fn run(mut self, mut commands: mpsc::Receiver<Command>, shutdown: CancellationToken) {
        let ids: Vec<_> = self
            .store
            .read()
            .profiles
            .into_iter()
            .filter(|p| p.auto_connect)
            .map(|p| p.id)
            .collect();
        for id in ids {
            let _ = self.start(&id, None).await;
        }
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                Some(command) = commands.recv() => {
                    let result = self.apply(command.request).await;
                    let _ = command.reply.send(result);
                }
            }
        }
        self.all_stop().await;
        shutdown.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn disconnect_batch_survives_a_worker_missing_its_cleanup_deadline() {
        let mut snapshot = Snapshot::default();
        let ids = vec!["finished".to_string(), "stuck".to_string()];
        for id in &ids {
            snapshot
                .connections
                .insert(id.clone(), ConnectionState::default());
        }
        let store = Store::new(snapshot);
        let (_tx, recovery) = watch::channel(Recovery::Ready);
        let mut manager = Manager::new(store.clone(), PathBuf::new(), recovery, false, false);
        manager.running.insert(
            ids[0].clone(),
            Running {
                cancel: CancellationToken::new(),
                task: tokio::spawn(async {}),
            },
        );
        manager.running.insert(
            ids[1].clone(),
            Running {
                cancel: CancellationToken::new(),
                task: tokio::spawn(std::future::pending()),
            },
        );
        manager.desired.extend(ids.iter().cloned());
        tokio::task::yield_now().await;
        manager.stop_many(&ids).await;
        assert!(manager.running.is_empty() && manager.desired.is_empty());
        assert!(
            store
                .read()
                .connections
                .values()
                .all(|c| c.state == "stopped")
        );
    }
}
