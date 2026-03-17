use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EntryKind {
    Incoming,
    Internal,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub message_id: String,
    pub received_at_ms: i64,
    pub sender: String,
    pub message_type: String,
    pub raw_payload: Vec<u8>,
    pub entry_kind: EntryKind,
}

pub struct LogStore {
    logs: RwLock<HashMap<String, Vec<LogEntry>>>,
    persistence_path: Option<PathBuf>,
}

impl Default for LogStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LogStore {
    pub fn new() -> Self {
        Self {
            logs: RwLock::new(HashMap::new()),
            persistence_path: None,
        }
    }

    pub fn with_persistence<P: AsRef<Path>>(dir: P) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let path = dir.join("logs.json");
        let logs = if path.exists() {
            match serde_json::from_slice(&fs::read(&path)?) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("warning: failed to deserialize logs from {}: {e}; starting with empty state", path.display());
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        Ok(Self {
            logs: RwLock::new(logs),
            persistence_path: Some(path),
        })
    }

    fn persist_map(path: &Path, logs: &HashMap<String, Vec<LogEntry>>) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(logs)?;
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, bytes)?;
        fs::rename(&tmp_path, path)
    }

    async fn persist_locked(&self, logs: &HashMap<String, Vec<LogEntry>>) -> std::io::Result<()> {
        if let Some(path) = &self.persistence_path {
            Self::persist_map(path, logs)?;
        }
        Ok(())
    }

    pub async fn persist_snapshot(&self) -> std::io::Result<()> {
        let guard = self.logs.read().await;
        self.persist_locked(&guard).await
    }

    pub async fn create_session_log(&self, session_id: &str) {
        let mut guard = self.logs.write().await;
        guard.entry(session_id.to_string()).or_default();
        let _ = self.persist_locked(&guard).await;
    }

    pub async fn append(&self, session_id: &str, entry: LogEntry) {
        let mut guard = self.logs.write().await;
        guard.entry(session_id.to_string()).or_default().push(entry);
        let _ = self.persist_locked(&guard).await;
    }

    pub async fn get_log(&self, session_id: &str) -> Option<Vec<LogEntry>> {
        let guard = self.logs.read().await;
        guard.get(session_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn entry(id: &str, kind: EntryKind) -> LogEntry {
        LogEntry {
            message_id: id.into(),
            received_at_ms: 1_700_000_000_000,
            sender: "test".into(),
            message_type: "Message".into(),
            raw_payload: vec![],
            entry_kind: kind,
        }
    }

    #[tokio::test]
    async fn create_append_get_round_trip() {
        let store = LogStore::new();
        store.create_session_log("s1").await;
        store.append("s1", entry("m1", EntryKind::Incoming)).await;
        store.append("s1", entry("m2", EntryKind::Incoming)).await;

        let log = store.get_log("s1").await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].message_id, "m1");
        assert_eq!(log[1].message_id, "m2");
    }

    #[tokio::test]
    async fn persistent_log_store_round_trip() {
        let base = std::env::temp_dir().join(format!(
            "macp-log-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = LogStore::with_persistence(&base).unwrap();
        store.append("s1", entry("m1", EntryKind::Incoming)).await;

        let reopened = LogStore::with_persistence(&base).unwrap();
        let log = reopened.get_log("s1").await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].message_id, "m1");
    }
}
