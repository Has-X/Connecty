use crate::model::{AgentError, Snapshot};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub struct Store {
    inner: Mutex<Snapshot>,
    pub changes: watch::Sender<u64>,
}
impl Store {
    pub fn new(snapshot: Snapshot) -> Arc<Self> {
        let (changes, _) = watch::channel(0);
        Arc::new(Self {
            inner: Mutex::new(snapshot),
            changes,
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
        self.store.update(|s| {
            if let Some(c) = s.connections.get_mut(&self.id)
                && c.generation == self.generation
            {
                c.log(level, code, message);
            }
        });
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
