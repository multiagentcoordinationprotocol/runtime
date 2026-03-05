use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Clone, Debug, PartialEq)]
pub enum EntryKind {
    Incoming,
    Internal,
}

#[derive(Clone, Debug)]
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
        }
    }

    /// Create an empty log for a session.
    pub async fn create_session_log(&self, session_id: &str) {
        let mut guard = self.logs.write().await;
        guard.entry(session_id.to_string()).or_default();
    }

    /// Append a log entry. Auto-creates the session log if it doesn't exist.
    pub async fn append(&self, session_id: &str, entry: LogEntry) {
        let mut guard = self.logs.write().await;
        guard.entry(session_id.to_string()).or_default().push(entry);
    }

    /// Get the log for a session. Returns None if session was never logged.
    pub async fn get_log(&self, session_id: &str) -> Option<Vec<LogEntry>> {
        let guard = self.logs.read().await;
        guard.get(session_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn auto_create_on_append() {
        let store = LogStore::new();
        // No explicit create_session_log call
        store.append("s2", entry("m1", EntryKind::Incoming)).await;

        let log = store.get_log("s2").await.unwrap();
        assert_eq!(log.len(), 1);
    }

    #[tokio::test]
    async fn ordering_preserved() {
        let store = LogStore::new();
        for i in 0..5 {
            store
                .append("s1", entry(&format!("m{}", i), EntryKind::Incoming))
                .await;
        }

        let log = store.get_log("s1").await.unwrap();
        for (i, e) in log.iter().enumerate() {
            assert_eq!(e.message_id, format!("m{}", i));
        }
    }

    #[tokio::test]
    async fn unknown_session_returns_none() {
        let store = LogStore::new();
        assert!(store.get_log("nonexistent").await.is_none());
    }
}
