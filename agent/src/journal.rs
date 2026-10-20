//! Bounded local metadata journal. Disk work runs off the SSH reactor, on demand.
use crate::model::{AgentError, LogEntry, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};

const CURRENT: &str = "Connecty-current.jsonl";
const PREVIOUS: &str = "Connecty-previous.jsonl";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    pub directory: PathBuf,
    pub keep_days: u32,
    pub max_mib: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub profile_id: String,
    pub profile: String,
    #[serde(flatten)]
    pub entry: LogEntry,
}
enum Work {
    Append(Record),
    Request(Value, oneshot::Sender<Result<Value, AgentError>>),
}
#[derive(Clone)]
pub struct Journal {
    tx: mpsc::Sender<Work>,
    dropped: Arc<AtomicU64>,
}
struct Disk {
    config: PathBuf,
    settings: Settings,
    error: Option<String>,
    cleaned_day: u64,
}
impl Journal {
    pub async fn start(dir: PathBuf) -> Self {
        let disk = tokio::task::spawn_blocking(move || {
            let config = dir.join("logging.json");
            let fallback = Settings {
                enabled: true,
                directory: dir.join("Logs"),
                keep_days: 7,
                max_mib: 2,
            };
            let read = (|| -> std::io::Result<Vec<u8>> {
                let file = std::fs::File::open(&config)?;
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut std::io::Read::take(file, 8193), &mut bytes)?;
                Ok(bytes)
            })();
            let (settings, error) = match read {
                Ok(bytes) if bytes.len() <= 8192 => {
                    match serde_json::from_slice::<Settings>(&bytes) {
                        Ok(s) if valid(&s) => (s, None),
                        _ => (
                            fallback,
                            Some(
                                "Invalid logging settings; using safe defaults until saved".into(),
                            ),
                        ),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (fallback, None),
                _ => (fallback, Some("Cannot read logging settings".into())),
            };
            Disk {
                config,
                settings,
                error,
                cleaned_day: 0,
            }
        })
        .await
        .expect("journal initialization worker");
        let (tx, mut rx) = mpsc::channel(128);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut count = dropped.clone();
        tokio::spawn(async move {
            let mut disk = disk;
            while let Some(work) = rx.recv().await {
                // No persistent file handle, polling timer, or per-connection disk thread.
                disk = match tokio::task::spawn_blocking(move || {
                    match work {
                        Work::Append(record) => {
                            if let Err(e) = disk.append(&record) {
                                disk.error = Some(e.to_string());
                            }
                        }
                        Work::Request(request, reply) => {
                            let result = disk.request(request, count.load(Ordering::Relaxed));
                            let _ = reply.send(result);
                        }
                    }
                    (disk, count)
                })
                .await
                {
                    Ok((next, returned_count)) => {
                        count = returned_count;
                        next
                    }
                    Err(_) => break,
                };
            }
        });
        Self { tx, dropped }
    }
    pub fn record(&self, record: Record) {
        if self.tx.try_send(Work::Append(record)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub async fn request(&self, value: Value) -> Result<Value, AgentError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Work::Request(value, tx))
            .await
            .map_err(|_| AgentError::config("Log worker unavailable"))?;
        rx.await
            .map_err(|_| AgentError::config("Log worker stopped"))?
    }
}
fn valid(s: &Settings) -> bool {
    let text = s.directory.to_string_lossy();
    s.directory.is_absolute()
        && !text.starts_with(r"\\")
        && text.len() <= 1024
        && !text.chars().any(char::is_control)
        && (1..=30).contains(&s.keep_days)
        && (1..=16).contains(&s.max_mib)
}
fn replace(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&temp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
impl Disk {
    fn records(&self, name: &str) -> std::io::Result<VecDeque<Record>> {
        let path = self.settings.directory.join(name);
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(VecDeque::new()),
            Err(e) => return Err(e),
        };
        // Never read arbitrary/unbounded data placed in the log folder.
        if file.metadata()?.len() > 16 * 1024 * 1024 + 4096 {
            return Err(std::io::Error::other("Log file exceeds safety limit"));
        }
        let mut out = VecDeque::new();
        let cutoff = now().saturating_sub(u64::from(self.settings.keep_days) * 86400);
        let mut reader = std::io::BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = std::io::Read::take(&mut reader, 4097).read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            if n > 4096 {
                return Err(std::io::Error::other("Invalid log record length"));
            }
            if let Ok(r) = serde_json::from_slice::<Record>(&line)
                && r.entry.time >= cutoff
                && r.profile_id.len() <= 80
                && r.profile.len() <= 160
                && r.entry.message.len() <= 512
                && r.entry.code.len() <= 64
                && r.entry.level.len() <= 16
            {
                if out.len() == 4096 {
                    out.pop_front();
                }
                out.push_back(r);
            }
        }
        Ok(out)
    }
    fn cleanup(&mut self) -> std::io::Result<()> {
        if self.cleaned_day == now() / 86400 {
            return Ok(());
        }
        for name in [PREVIOUS, CURRENT] {
            let path = self.settings.directory.join(name);
            if !path.exists() {
                continue;
            }
            let records = self.records(name)?;
            let mut bytes = Vec::new();
            for r in records {
                serde_json::to_writer(&mut bytes, &r)?;
                bytes.push(b'\n');
            }
            let max = self.settings.max_mib as usize * 1024 * 1024;
            if bytes.len() > max {
                let start = bytes.len() - max;
                let end = bytes[start..]
                    .iter()
                    .position(|b| *b == b'\n')
                    .map(|n| start + n + 1)
                    .unwrap_or(bytes.len());
                bytes.drain(..end);
            }
            replace(&path, &bytes)?;
        }
        self.cleaned_day = now() / 86400;
        Ok(())
    }
    fn append(&mut self, record: &Record) -> std::io::Result<()> {
        if !self.settings.enabled {
            return Ok(());
        }
        std::fs::create_dir_all(&self.settings.directory)?;
        self.cleanup()?;
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        let current = self.settings.directory.join(CURRENT);
        if std::fs::metadata(&current).map(|m| m.len()).unwrap_or(0) + bytes.len() as u64
            > u64::from(self.settings.max_mib) * 1024 * 1024
        {
            let previous = self.settings.directory.join(PREVIOUS);
            if previous.exists() {
                std::fs::remove_file(&previous)?;
            }
            std::fs::rename(&current, previous)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(current)?
            .write_all(&bytes)
    }
    fn request(&mut self, value: Value, dropped: u64) -> Result<Value, AgentError> {
        let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
            if value["op"] == "logging" {
                let next: Settings = serde_json::from_value(value["settings"].clone())?;
                if !valid(&next) {
                    return Err(
                        "Use a local absolute folder, 1–30 days and 1–16 MiB per log file".into(),
                    );
                }
                std::fs::create_dir_all(&next.directory)?;
                // Test access before changing the saved location. Old folders are left intact.
                let probe = next
                    .directory
                    .join(format!(".connecty-write-test-{}", std::process::id()));
                std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&probe)?;
                std::fs::remove_file(probe)?;
                replace(&self.config, &serde_json::to_vec_pretty(&next)?)?;
                self.settings = next;
                self.cleaned_day = 0;
                self.error = None;
            }
            if value["op"] == "history" {
                self.cleanup()?;
            }
            let mut records = VecDeque::new();
            if value["op"] == "history" {
                let id = value["id"].as_str().unwrap_or("");
                for name in [PREVIOUS, CURRENT] {
                    for r in self.records(name)? {
                        if id.is_empty() || r.profile_id == id {
                            if records.len() == 256 {
                                records.pop_front();
                            }
                            records.push_back(r);
                        }
                    }
                }
            }
            Ok(
                json!({"settings":self.settings,"records":records,"error":self.error,"dropped":dropped}),
            )
        })();
        result.map_err(|e| AgentError::new("logging", e.to_string(), false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_age_cleanup_and_disabled_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut disk = Disk {
            config: dir.path().join("logging.json"),
            settings: Settings {
                enabled: true,
                directory: dir.path().into(),
                keep_days: 7,
                max_mib: 1,
            },
            error: None,
            cleaned_day: now() / 86400,
        };
        let make = |time| Record {
            profile_id: "one".into(),
            profile: "Test".into(),
            entry: LogEntry {
                time,
                level: "info".into(),
                code: "stream_open".into(),
                message: "Peer connected".into(),
            },
        };
        std::fs::write(dir.path().join(CURRENT), vec![b'\n'; 1024 * 1024]).unwrap();
        disk.append(&make(now())).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path().join(PREVIOUS)).unwrap().len(),
            1024 * 1024
        );
        assert!(std::fs::metadata(dir.path().join(CURRENT)).unwrap().len() < 1024);
        disk.append(&make(1)).unwrap();
        disk.cleaned_day = 0;
        disk.cleanup().unwrap();
        assert_eq!(disk.records(CURRENT).unwrap().len(), 1);
        let before = std::fs::read(dir.path().join(CURRENT)).unwrap();
        disk.settings.enabled = false;
        disk.append(&make(now())).unwrap();
        assert_eq!(std::fs::read(dir.path().join(CURRENT)).unwrap(), before);
        assert!(!valid(&Settings {
            directory: "relative".into(),
            ..disk.settings
        }));
    }
    #[tokio::test]
    async fn persistence_filter_retention_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::start(dir.path().into()).await;
        for (id, time) in [("one", now()), ("two", now()), ("one", 1)] {
            journal.record(Record {
                profile_id: id.into(),
                profile: "Test".into(),
                entry: LogEntry {
                    time,
                    level: "info".into(),
                    code: "stream_open".into(),
                    message: "127.0.0.1:1234 connected".into(),
                },
            });
        }
        let history = journal
            .request(json!({"op":"history","id":"one"}))
            .await
            .unwrap();
        assert_eq!(history["records"].as_array().unwrap().len(), 1);
        let mut settings = history["settings"].clone();
        settings["enabled"] = json!(false);
        journal
            .request(json!({"op":"logging","settings":settings}))
            .await
            .unwrap();
        let reopened = Journal::start(dir.path().into()).await;
        assert_eq!(
            reopened.request(json!({"op":"history"})).await.unwrap()["settings"]["enabled"],
            false
        );
        settings["keepDays"] = json!(0);
        assert!(
            journal
                .request(json!({"op":"logging","settings":settings}))
                .await
                .is_err()
        );
    }
}
