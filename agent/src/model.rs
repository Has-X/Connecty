use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_PROFILES: usize = 32;
pub const MAX_LOGS: usize = 32;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_STREAMS: usize = 64;
fn bounded(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    #[default]
    Reverse,
    Local,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub identity_file: String,
    #[serde(default)]
    pub known_hosts: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default = "loopback")]
    pub bind_address: String,
    #[serde(default = "loopback")]
    pub target: String,
    pub remote_port: u16,
    pub target_port: u16,
    #[serde(default = "yes")]
    pub retry: bool,
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default)]
    pub probe: String,
}
fn loopback() -> String {
    "127.0.0.1".into()
}
fn yes() -> bool {
    true
}
impl Profile {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.id.is_empty()
            || self.id.len() > 80
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "{}-_".contains(c))
        {
            return Err(AgentError::config("Invalid connection ID"));
        }
        if self.name.trim().is_empty()
            || self.name.len() > 160
            || self.host.trim().is_empty()
            || self.host.len() > 253
            || self.host.starts_with('-')
        {
            return Err(AgentError::config("A connection needs a name and SSH host"));
        }
        if !self
            .host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_@[]:".contains(c))
            || self.target.is_empty()
            || self.target.len() > 253
            || self
                .target
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(AgentError::config("Invalid host or target address"));
        }
        if self.remote_port == 0 || self.target_port == 0 {
            return Err(AgentError::config("Ports must be 1–65535"));
        }
        if self.bind_address != "127.0.0.1" && self.bind_address != "::1" {
            return Err(AgentError::config("Listeners must use a loopback address"));
        }
        if self.probe.len() > 2048 || self.probe.contains(['\r', '\n', '\0']) {
            return Err(AgentError::config(
                "The probe must be one command, at most 2048 bytes",
            ));
        }
        if self.hostname.len() > 253
            || self.user.len() > 128
            || self.identity_file.len() > 1024
            || self.known_hosts.len() > 1024
        {
            return Err(AgentError::config(
                "Connection field exceeds its size limit",
            ));
        }
        if [
            &self.name,
            &self.hostname,
            &self.user,
            &self.identity_file,
            &self.known_hosts,
        ]
        .iter()
        .any(|s| s.chars().any(char::is_control))
        {
            return Err(AgentError::config(
                "Connection fields cannot contain control characters",
            ));
        }
        Ok(())
    }
}
pub fn validate_profiles(profiles: &[Profile]) -> Result<(), AgentError> {
    if profiles.len() > MAX_PROFILES {
        return Err(AgentError::config("At most 32 connections are supported"));
    }
    for (i, p) in profiles.iter().enumerate() {
        p.validate()?;
        if profiles[..i].iter().any(|q| {
            q.id == p.id
                || (q.direction == p.direction
                    && q.remote_port == p.remote_port
                    && (p.direction == Direction::Local || q.host.eq_ignore_ascii_case(&p.host)))
        }) {
            return Err(AgentError::config(
                "Duplicate connection ID or listening endpoint",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
impl AgentError {
    pub fn new(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: bounded(
                &message
                    .into()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(512)
                    .collect::<String>(),
                512,
            ),
            retryable,
        }
    }
    pub fn config(message: impl Into<String>) -> Self {
        Self::new("configuration", message, false)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new("network", message, true)
    }
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    #[serde(skip)]
    pub generation: u64,
    pub state: String,
    pub detail: String,
    pub retries: u32,
    pub error: Option<AgentError>,
    pub verified_at: Option<u64>,
    pub since: u64,
    pub logs: VecDeque<LogEntry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub time: u64,
    pub level: String,
    pub code: String,
    pub message: String,
}
impl ConnectionState {
    pub fn set(&mut self, state: &str, detail: &str, error: Option<AgentError>) {
        let healthy = |s: &str| s == "forwarding" || s == "verified";
        if self.state != state && !(healthy(&self.state) && healthy(state)) {
            self.since = now();
        }
        self.state = state.into();
        self.detail = detail.into();
        self.error = error;
        if state != "verified" {
            self.verified_at = None;
        }
    }
    pub fn log(&mut self, level: &str, code: &str, message: &str) {
        if self.logs.len() == MAX_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            time: now(),
            level: level.into(),
            code: code.into(),
            message: bounded(
                &message
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect::<String>(),
                256,
            ),
        });
    }
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: u32,
    pub agent_pid: u32,
    pub data_directory: String,
    pub startup: bool,
    pub profiles: Vec<Profile>,
    pub connections: BTreeMap<String, ConnectionState>,
    pub error: Option<AgentError>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            agent_pid: std::process::id(),
            data_directory: String::new(),
            startup: false,
            profiles: vec![],
            connections: BTreeMap::new(),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn profile() -> Profile {
        serde_json::from_str(
            r#"{"id":"test","name":"Test","host":"example","remotePort":22023,"targetPort":22}"#,
        )
        .unwrap()
    }
    #[test]
    fn legacy_and_private_defaults() {
        let p = profile();
        p.validate().unwrap();
        assert_eq!(p.direction, Direction::Reverse);
        assert_eq!(p.bind_address, "127.0.0.1");
    }
    #[test]
    fn reject_invalid_and_duplicate() {
        let mut p = profile();
        p.bind_address = "0.0.0.0".into();
        assert!(p.validate().is_err());
        assert!(validate_profiles(&[profile(), profile()]).is_err());
    }
    #[test]
    fn bounded_structured_logs() {
        let mut s = ConnectionState::default();
        for _ in 0..500 {
            s.log("info", "x", &"z".repeat(1000));
        }
        assert_eq!(s.logs.len(), MAX_LOGS);
        assert_eq!(s.logs[0].message.len(), 256);
        s.log("info", "x", &"😀".repeat(300));
        assert_eq!(s.logs.back().unwrap().message.len(), 256);
    }
    #[test]
    fn maximum_snapshot_fits_pipe_frame() {
        let mut snapshot = Snapshot::default();
        for n in 0..MAX_PROFILES {
            let mut p = profile();
            p.id = format!("{n:02}{}", "x".repeat(78));
            p.host = format!("{n:02}{}", "x".repeat(251));
            p.name = "\"".repeat(160);
            p.hostname = "\"".repeat(253);
            p.user = "\"".repeat(128);
            p.identity_file = "\"".repeat(1024);
            p.known_hosts = "\"".repeat(1024);
            p.probe = "\"".repeat(2048);
            p.target = "\"".repeat(253);
            let mut state = ConnectionState::default();
            let error = AgentError::network("\"".repeat(512));
            state.set("retrying", &error.message, Some(error.clone()));
            for _ in 0..MAX_LOGS {
                state.log("warn", "verification_failed", &"\"".repeat(256));
            }
            snapshot.connections.insert(p.id.clone(), state);
            snapshot.profiles.push(p);
        }
        validate_profiles(&snapshot.profiles).unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({"event":"reply","requestId":u64::MAX,"ok":false,"error":AgentError::network("\"".repeat(512)),"snapshot":snapshot})).unwrap();
        assert!(
            bytes.len() <= MAX_FRAME,
            "Snapshot is {} bytes",
            bytes.len()
        );
    }
}
