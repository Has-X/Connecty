use crate::model::{AgentError, Snapshot};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub struct Store {
    inner: Mutex<Snapshot>,
    pub changes: watch::Sender<u64>,
    pub journal: std::sync::OnceLock<crate::journal::Journal>,
}
impl Store {
    pub fn new(snapshot: Snapshot) -> Arc<Self> {
        let (changes, _) = watch::channel(0);
        Arc::new(Self {
            inner: Mutex::new(snapshot),
            changes,
            journal: std::sync::OnceLock::new(),
        })
    }
    pub fn read(&self) -> Snapshot {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.inner.lock().unwrap_or_else(|p| p.into_inner()));
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
}
#[derive(Clone)]
pub struct Publisher {
    pub store: Arc<Store>,
    pub id: String,
    pub generation: u64,
}
impl Publisher {
    pub fn status(&self, state: &str, detail: &str, error: Option<AgentError>) {
        self.store.update(|s| {
            if let Some(c) = s.connections.get_mut(&self.id)
                && c.generation == self.generation
            {
                c.set(state, detail, error);
                if state == "verified" {
                    c.verified_at = Some(crate::model::now());
                }
            }
        });
    }
    pub fn log(&self, level: &str, code: &str, message: &str) {
        let mut record = None;
        self.store.update(|s| {
            if let Some(c) = s.connections.get_mut(&self.id)
                && c.generation == self.generation
            {
                c.log(level, code, message);
                if let Some(entry) = c.logs.back().cloned() {
                    record = Some(entry);
                }
            }
        });
        if let Some(entry) = record
            && let Some(journal) = self.store.journal.get()
        {
            let snapshot = self.store.inner.lock().unwrap_or_else(|p| p.into_inner());
            let name = snapshot
                .profiles
                .iter()
                .find(|p| p.id == self.id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            journal.record(crate::journal::Record {
                profile_id: self.id.clone(),
                profile: name,
                entry,
            });
        }
    }
    pub fn retry(&self) {
        self.store.update(|s| {
            if let Some(c) = s.connections.get_mut(&self.id)
                && c.generation == self.generation
            {
                c.retries = c.retries.saturating_add(1);
            }
        });
    }
}
