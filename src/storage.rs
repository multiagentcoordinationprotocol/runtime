use crate::log_store::LogEntry;
use crate::registry::PersistedSession;
use crate::session::Session;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs as tfs;
use tokio::io::AsyncWriteExt;

const STORAGE_VERSION: u32 = 3;

// ---------------------------------------------------------------------------
// StorageBackend trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync {
    async fn save_session(&self, session: &Session) -> io::Result<()>;
    async fn load_session(&self, session_id: &str) -> io::Result<Option<Session>>;
    async fn load_all_sessions(&self) -> io::Result<Vec<Session>>;
    async fn append_log_entry(&self, session_id: &str, entry: &LogEntry) -> io::Result<()>;
    async fn load_log(&self, session_id: &str) -> io::Result<Vec<LogEntry>>;
    async fn create_session_storage(&self, session_id: &str) -> io::Result<()>;
}

// ---------------------------------------------------------------------------
// MemoryBackend — used for MACP_MEMORY_ONLY=1 and tests
// ---------------------------------------------------------------------------

pub struct MemoryBackend;

#[async_trait::async_trait]
impl StorageBackend for MemoryBackend {
    async fn save_session(&self, _session: &Session) -> io::Result<()> {
        Ok(())
    }

    async fn load_session(&self, _session_id: &str) -> io::Result<Option<Session>> {
        Ok(None)
    }

    async fn load_all_sessions(&self) -> io::Result<Vec<Session>> {
        Ok(vec![])
    }

    async fn append_log_entry(&self, _session_id: &str, _entry: &LogEntry) -> io::Result<()> {
        Ok(())
    }

    async fn load_log(&self, _session_id: &str) -> io::Result<Vec<LogEntry>> {
        Ok(vec![])
    }

    async fn create_session_storage(&self, _session_id: &str) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FileBackend — per-session directory structure with append-only JSONL logs
// ---------------------------------------------------------------------------

pub struct FileBackend {
    base_dir: PathBuf,
}

impl FileBackend {
    pub fn new(base_dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(base_dir.join("sessions"))?;
        Ok(Self { base_dir })
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.base_dir.join("sessions").join(session_id)
    }

    fn session_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session.json")
    }

    fn log_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("log.jsonl")
    }

    async fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
        let tmp_path = path.with_extension("json.tmp");
        tfs::write(&tmp_path, data).await?;
        tfs::rename(&tmp_path, path).await
    }
}

#[async_trait::async_trait]
impl StorageBackend for FileBackend {
    async fn create_session_storage(&self, session_id: &str) -> io::Result<()> {
        tfs::create_dir_all(self.session_dir(session_id)).await
    }

    async fn save_session(&self, session: &Session) -> io::Result<()> {
        let persisted = PersistedSession::from(session);
        let bytes = serde_json::to_vec_pretty(&persisted)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::atomic_write(&self.session_file(&session.session_id), &bytes).await
    }

    async fn load_session(&self, session_id: &str) -> io::Result<Option<Session>> {
        let path = self.session_file(session_id);
        if tfs::metadata(&path).await.is_err() {
            return Ok(None);
        }
        let bytes = tfs::read(&path).await?;
        let persisted: PersistedSession = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(Session::from(persisted)))
    }

    async fn load_all_sessions(&self) -> io::Result<Vec<Session>> {
        let sessions_dir = self.base_dir.join("sessions");
        if tfs::metadata(&sessions_dir).await.is_err() {
            return Ok(vec![]);
        }
        let mut sessions = Vec::new();
        let mut entries = tfs::read_dir(&sessions_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let session_file = entry.path().join("session.json");
            if tfs::metadata(&session_file).await.is_err() {
                continue;
            }
            let bytes = tfs::read(&session_file).await?;
            match serde_json::from_slice::<PersistedSession>(&bytes) {
                Ok(persisted) => sessions.push(Session::from(persisted)),
                Err(e) => {
                    eprintln!(
                        "warning: failed to deserialize session from {}: {e}; skipping",
                        session_file.display()
                    );
                }
            }
        }
        Ok(sessions)
    }

    async fn append_log_entry(&self, session_id: &str, entry: &LogEntry) -> io::Result<()> {
        let path = self.log_file(session_id);
        let mut line = serde_json::to_string(entry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push('\n');

        let mut file = tfs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.sync_data().await?;
        Ok(())
    }

    async fn load_log(&self, session_id: &str) -> io::Result<Vec<LogEntry>> {
        let path = self.log_file(session_id);
        if tfs::metadata(&path).await.is_err() {
            return Ok(vec![]);
        }
        let content = tfs::read_to_string(&path).await?;
        let mut entries = Vec::new();
        for (line_num, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<LogEntry>(line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    eprintln!(
                        "warning: failed to parse log entry at {}:{}: {e}; skipping",
                        path.display(),
                        line_num + 1
                    );
                }
            }
        }
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// storage_version.json
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct StorageVersion {
    version: u32,
}

pub fn write_storage_version(base_dir: &Path) -> io::Result<()> {
    let sv = StorageVersion {
        version: STORAGE_VERSION,
    };
    let bytes = serde_json::to_vec_pretty(&sv)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(base_dir.join("storage_version.json"), bytes)
}

pub fn read_storage_version(base_dir: &Path) -> io::Result<Option<u32>> {
    let path = base_dir.join("storage_version.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    let sv: StorageVersion = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(sv.version))
}

// ---------------------------------------------------------------------------
// Migration from legacy monolithic format (v1 → v2)
// ---------------------------------------------------------------------------

pub fn migrate_if_needed(base_dir: &Path) -> io::Result<()> {
    let sessions_dir = base_dir.join("sessions");
    let legacy_sessions = base_dir.join("sessions.json");
    let legacy_logs = base_dir.join("logs.json");

    // Already at current version or fresh install (no legacy files, sessions dir exists)
    let current_version = read_storage_version(base_dir)?;
    if sessions_dir.exists() && !legacy_sessions.exists() && !legacy_logs.exists() {
        // v2 → v3: no-op data migration, just bump version.  New LogEntry fields
        // use #[serde(default)] so existing v2 JSONL lines deserialize fine.
        if current_version.unwrap_or(0) < STORAGE_VERSION {
            write_storage_version(base_dir)?;
        }
        return Ok(());
    }

    if !legacy_sessions.exists() && !legacy_logs.exists() && !sessions_dir.exists() {
        write_storage_version(base_dir)?;
        return Ok(());
    }

    // Already migrated from v1
    if sessions_dir.exists() && !legacy_sessions.exists() && !legacy_logs.exists() {
        write_storage_version(base_dir)?;
        return Ok(());
    }

    println!("Migrating legacy storage format to per-session directories...");

    // Load legacy sessions
    let sessions: HashMap<String, PersistedSession> = if legacy_sessions.exists() {
        let bytes = fs::read(&legacy_sessions)?;
        serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse legacy sessions.json: {e}"),
            )
        })?
    } else {
        HashMap::new()
    };

    // Load legacy logs
    let logs: HashMap<String, Vec<LogEntry>> = if legacy_logs.exists() {
        let bytes = fs::read(&legacy_logs)?;
        serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse legacy logs.json: {e}"),
            )
        })?
    } else {
        HashMap::new()
    };

    fs::create_dir_all(&sessions_dir)?;

    // Migrate each session
    for (session_id, persisted) in &sessions {
        let dir = sessions_dir.join(session_id);
        fs::create_dir_all(&dir)?;

        // Write session.json
        let session_bytes = serde_json::to_vec_pretty(persisted)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(dir.join("session.json"), session_bytes)?;

        // Write log.jsonl
        if let Some(entries) = logs.get(session_id) {
            let mut log_data = String::new();
            for entry in entries {
                let line = serde_json::to_string(entry)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                log_data.push_str(&line);
                log_data.push('\n');
            }
            fs::write(dir.join("log.jsonl"), log_data)?;
        }
    }

    // Also migrate logs for sessions that only appear in logs (not in sessions.json)
    for (session_id, entries) in &logs {
        if sessions.contains_key(session_id) {
            continue;
        }
        let dir = sessions_dir.join(session_id);
        fs::create_dir_all(&dir)?;
        let mut log_data = String::new();
        for entry in entries {
            let line = serde_json::to_string(entry)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            log_data.push_str(&line);
            log_data.push('\n');
        }
        fs::write(dir.join("log.jsonl"), log_data)?;
    }

    // Backup old files instead of deleting
    if legacy_sessions.exists() {
        fs::rename(&legacy_sessions, base_dir.join("sessions.json.migrated"))?;
    }
    if legacy_logs.exists() {
        fs::rename(&legacy_logs, base_dir.join("logs.json.migrated"))?;
    }

    write_storage_version(base_dir)?;
    println!(
        "Migration complete: {} sessions, {} log sets migrated.",
        sessions.len(),
        logs.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Crash recovery
// ---------------------------------------------------------------------------

pub fn recover_session(session: &mut Session, log_entries: &[LogEntry]) {
    // Ensure all log entry message IDs are in the session's dedup set.
    // If the runtime crashed after writing a log entry but before persisting
    // the session snapshot, there will be entries in the log not reflected
    // in seen_message_ids.
    let mut recovered = 0usize;
    for entry in log_entries {
        if !entry.message_id.is_empty() && session.seen_message_ids.insert(entry.message_id.clone())
        {
            recovered += 1;
        }
    }
    if recovered > 0 {
        eprintln!(
            "recovery: session '{}' reconciled {} log entries into dedup state",
            session.session_id, recovered
        );
    }
}

pub fn cleanup_temp_files(base_dir: &Path) {
    let sessions_dir = base_dir.join("sessions");
    if !sessions_dir.exists() {
        return;
    }
    if let Ok(entries) = fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = entry.path();
            if let Ok(files) = fs::read_dir(&dir) {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                        eprintln!("recovery: removing orphaned temp file {}", path.display());
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_store::EntryKind;
    use crate::session::SessionState;
    use std::collections::HashSet;

    fn sample_session(id: &str) -> Session {
        Session {
            session_id: id.into(),
            state: SessionState::Open,
            ttl_expiry: 61_000,
            ttl_ms: 60_000,
            started_at_unix_ms: 1_000,
            resolution: None,
            mode: "macp.mode.decision.v1".into(),
            mode_state: vec![1, 2, 3],
            participants: vec!["alice".into(), "bob".into()],
            seen_message_ids: HashSet::from(["m1".into()]),
            intent: "test intent".into(),
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "pol-1".into(),
            context: vec![9],
            roots: vec![crate::pb::Root {
                uri: "root://1".into(),
                name: "r1".into(),
            }],
            initiator_sender: "alice".into(),
        }
    }

    fn sample_entry(id: &str) -> LogEntry {
        LogEntry {
            message_id: id.into(),
            received_at_ms: 1_700_000_000_000,
            sender: "alice".into(),
            message_type: "Message".into(),
            raw_payload: vec![],
            entry_kind: EntryKind::Incoming,
            session_id: String::new(),
            mode: String::new(),
            macp_version: String::new(),
        }
    }

    #[tokio::test]
    async fn file_backend_session_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf()).unwrap();

        let session = sample_session("s1");
        backend.create_session_storage("s1").await.unwrap();
        backend.save_session(&session).await.unwrap();

        let loaded = backend.load_session("s1").await.unwrap().unwrap();
        assert_eq!(loaded.session_id, "s1");
        assert_eq!(loaded.ttl_ms, 60_000);
        assert_eq!(loaded.mode_version, "1.0.0");
        assert!(loaded.seen_message_ids.contains("m1"));
        assert_eq!(loaded.participants, vec!["alice", "bob"]);
    }

    #[tokio::test]
    async fn file_backend_log_append_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf()).unwrap();

        backend.create_session_storage("s1").await.unwrap();
        backend
            .append_log_entry("s1", &sample_entry("m1"))
            .await
            .unwrap();
        backend
            .append_log_entry("s1", &sample_entry("m2"))
            .await
            .unwrap();
        backend
            .append_log_entry("s1", &sample_entry("m3"))
            .await
            .unwrap();

        let log = backend.load_log("s1").await.unwrap();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].message_id, "m1");
        assert_eq!(log[1].message_id, "m2");
        assert_eq!(log[2].message_id, "m3");
    }

    #[tokio::test]
    async fn file_backend_load_all_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf()).unwrap();

        for id in ["s1", "s2", "s3"] {
            backend.create_session_storage(id).await.unwrap();
            backend.save_session(&sample_session(id)).await.unwrap();
        }

        let mut sessions = backend.load_all_sessions().await.unwrap();
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].session_id, "s1");
        assert_eq!(sessions[1].session_id, "s2");
        assert_eq!(sessions[2].session_id, "s3");
    }

    #[tokio::test]
    async fn memory_backend_is_noop() {
        let backend = MemoryBackend;
        backend.create_session_storage("s1").await.unwrap();
        backend.save_session(&sample_session("s1")).await.unwrap();
        assert!(backend.load_session("s1").await.unwrap().is_none());
        assert!(backend.load_all_sessions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn append_only_no_full_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf()).unwrap();
        backend.create_session_storage("s1").await.unwrap();

        // Append 100 entries
        for i in 0..100 {
            backend
                .append_log_entry("s1", &sample_entry(&format!("m{}", i)))
                .await
                .unwrap();
        }

        // Verify file has 100 lines
        let content = fs::read_to_string(backend.log_file("s1")).unwrap();
        let line_count = content.lines().count();
        assert_eq!(line_count, 100);

        // Verify all entries are loadable
        let log = backend.load_log("s1").await.unwrap();
        assert_eq!(log.len(), 100);
    }

    #[test]
    fn crash_recovery_reconciles_dedup_state() {
        let mut session = sample_session("s1");
        // session has "m1" in seen_message_ids
        assert!(session.seen_message_ids.contains("m1"));
        assert!(!session.seen_message_ids.contains("m2"));
        assert!(!session.seen_message_ids.contains("m3"));

        let entries = vec![
            sample_entry("m1"), // already in dedup set
            sample_entry("m2"), // missing from dedup set
            sample_entry("m3"), // missing from dedup set
        ];

        recover_session(&mut session, &entries);

        assert!(session.seen_message_ids.contains("m1"));
        assert!(session.seen_message_ids.contains("m2"));
        assert!(session.seen_message_ids.contains("m3"));
    }

    #[tokio::test]
    async fn migration_from_legacy_format() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        // Create legacy format files
        let session = sample_session("s1");
        let persisted = PersistedSession::from(&session);
        let sessions_map: HashMap<String, PersistedSession> =
            [("s1".into(), persisted)].into_iter().collect();
        fs::write(
            base.join("sessions.json"),
            serde_json::to_vec_pretty(&sessions_map).unwrap(),
        )
        .unwrap();

        let entries = vec![sample_entry("m1"), sample_entry("m2")];
        let logs_map: HashMap<String, Vec<LogEntry>> =
            [("s1".into(), entries)].into_iter().collect();
        fs::write(
            base.join("logs.json"),
            serde_json::to_vec_pretty(&logs_map).unwrap(),
        )
        .unwrap();

        // Run migration
        migrate_if_needed(base).unwrap();

        // Verify per-session directories created
        assert!(base.join("sessions/s1/session.json").exists());
        assert!(base.join("sessions/s1/log.jsonl").exists());

        // Verify old files renamed
        assert!(base.join("sessions.json.migrated").exists());
        assert!(base.join("logs.json.migrated").exists());
        assert!(!base.join("sessions.json").exists());
        assert!(!base.join("logs.json").exists());

        // Verify storage version
        assert_eq!(read_storage_version(base).unwrap(), Some(STORAGE_VERSION));

        // Verify data is loadable via FileBackend
        let backend = FileBackend::new(base.to_path_buf()).unwrap();
        let loaded = backend.load_session("s1").await.unwrap().unwrap();
        assert_eq!(loaded.session_id, "s1");
        assert_eq!(loaded.ttl_ms, 60_000);

        let log = backend.load_log("s1").await.unwrap();
        assert_eq!(log.len(), 2);
    }

    #[tokio::test]
    async fn ttl_ms_backward_compat_deserialization() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let backend = FileBackend::new(base.to_path_buf()).unwrap();
        backend.create_session_storage("s1").await.unwrap();

        // Write a session JSON without ttl_ms (simulating old format)
        let json = serde_json::json!({
            "session_id": "s1",
            "state": "Open",
            "ttl_expiry": 61000,
            "started_at_unix_ms": 1000,
            "resolution": null,
            "mode": "macp.mode.decision.v1",
            "mode_state": [],
            "participants": ["alice"],
            "seen_message_ids": [],
            "intent": "",
            "mode_version": "1.0.0",
            "configuration_version": "cfg",
            "policy_version": "pol",
            "context": [],
            "roots": [],
            "initiator_sender": "alice"
        });
        fs::write(
            backend.session_file("s1"),
            serde_json::to_vec_pretty(&json).unwrap(),
        )
        .unwrap();

        let loaded = backend.load_session("s1").await.unwrap().unwrap();
        // ttl_ms should be computed from ttl_expiry - started_at_unix_ms
        assert_eq!(loaded.ttl_ms, 60_000);
    }

    #[test]
    fn cleanup_temp_files_removes_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let sessions_dir = base.join("sessions").join("s1");
        fs::create_dir_all(&sessions_dir).unwrap();

        // Create an orphaned temp file
        fs::write(sessions_dir.join("session.json.tmp"), b"partial").unwrap();
        assert!(sessions_dir.join("session.json.tmp").exists());

        cleanup_temp_files(base);

        assert!(!sessions_dir.join("session.json.tmp").exists());
    }

    #[tokio::test]
    async fn write_ordering_log_before_session() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf()).unwrap();
        backend.create_session_storage("s1").await.unwrap();

        // Write log entry (commit point)
        backend
            .append_log_entry("s1", &sample_entry("m1"))
            .await
            .unwrap();

        // Verify log is durable even without session save
        let log = backend.load_log("s1").await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].message_id, "m1");

        // Session load should return None (not yet saved)
        assert!(backend.load_session("s1").await.unwrap().is_none());

        // Now save session
        backend.save_session(&sample_session("s1")).await.unwrap();
        assert!(backend.load_session("s1").await.unwrap().is_some());
    }
}
