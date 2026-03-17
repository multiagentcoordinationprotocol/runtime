use crate::session::Session;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedRoot {
    uri: String,
    name: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedSession {
    session_id: String,
    state: crate::session::SessionState,
    ttl_expiry: i64,
    started_at_unix_ms: i64,
    resolution: Option<Vec<u8>>,
    mode: String,
    mode_state: Vec<u8>,
    participants: Vec<String>,
    seen_message_ids: Vec<String>,
    intent: String,
    mode_version: String,
    configuration_version: String,
    policy_version: String,
    context: Vec<u8>,
    roots: Vec<PersistedRoot>,
    initiator_sender: String,
}

impl From<&Session> for PersistedSession {
    fn from(session: &Session) -> Self {
        Self {
            session_id: session.session_id.clone(),
            state: session.state.clone(),
            ttl_expiry: session.ttl_expiry,
            started_at_unix_ms: session.started_at_unix_ms,
            resolution: session.resolution.clone(),
            mode: session.mode.clone(),
            mode_state: session.mode_state.clone(),
            participants: session.participants.clone(),
            seen_message_ids: session.seen_message_ids.iter().cloned().collect(),
            intent: session.intent.clone(),
            mode_version: session.mode_version.clone(),
            configuration_version: session.configuration_version.clone(),
            policy_version: session.policy_version.clone(),
            context: session.context.clone(),
            roots: session
                .roots
                .iter()
                .map(|root| PersistedRoot {
                    uri: root.uri.clone(),
                    name: root.name.clone(),
                })
                .collect(),
            initiator_sender: session.initiator_sender.clone(),
        }
    }
}

impl From<PersistedSession> for Session {
    fn from(session: PersistedSession) -> Self {
        Self {
            session_id: session.session_id,
            state: session.state,
            ttl_expiry: session.ttl_expiry,
            started_at_unix_ms: session.started_at_unix_ms,
            resolution: session.resolution,
            mode: session.mode,
            mode_state: session.mode_state,
            participants: session.participants,
            seen_message_ids: session.seen_message_ids.into_iter().collect(),
            intent: session.intent,
            mode_version: session.mode_version,
            configuration_version: session.configuration_version,
            policy_version: session.policy_version,
            context: session.context,
            roots: session
                .roots
                .into_iter()
                .map(|root| crate::pb::Root {
                    uri: root.uri,
                    name: root.name,
                })
                .collect(),
            initiator_sender: session.initiator_sender,
        }
    }
}

pub struct SessionRegistry {
    pub(crate) sessions: RwLock<HashMap<String, Session>>,
    persistence_path: Option<PathBuf>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            persistence_path: None,
        }
    }

    pub fn with_persistence<P: AsRef<Path>>(dir: P) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let path = dir.join("sessions.json");
        let sessions = Self::load_sessions(&path)?;
        Ok(Self {
            sessions: RwLock::new(sessions),
            persistence_path: Some(path),
        })
    }

    fn load_sessions(path: &Path) -> std::io::Result<HashMap<String, Session>> {
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let bytes = fs::read(path)?;
        let persisted: HashMap<String, PersistedSession> = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warning: failed to deserialize sessions from {}: {e}; starting with empty state", path.display());
                HashMap::new()
            }
        };
        Ok(persisted
            .into_iter()
            .map(|(id, session)| (id, session.into()))
            .collect())
    }

    fn persist_map(path: &Path, sessions: &HashMap<String, Session>) -> std::io::Result<()> {
        let persisted: HashMap<String, PersistedSession> = sessions
            .iter()
            .map(|(id, session)| (id.clone(), PersistedSession::from(session)))
            .collect();
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, bytes)?;
        fs::rename(&tmp_path, path)
    }

    pub(crate) async fn persist_locked(
        &self,
        sessions: &HashMap<String, Session>,
    ) -> std::io::Result<()> {
        if let Some(path) = &self.persistence_path {
            Self::persist_map(path, sessions)?;
        }
        Ok(())
    }

    pub async fn persist_snapshot(&self) -> std::io::Result<()> {
        let guard = self.sessions.read().await;
        self.persist_locked(&guard).await
    }

    pub async fn get_session(&self, session_id: &str) -> Option<Session> {
        let guard = self.sessions.read().await;
        guard.get(session_id).cloned()
    }

    pub async fn insert_session_for_test(&self, session_id: String, session: Session) {
        let mut guard = self.sessions.write().await;
        guard.insert(session_id, session);
        let _ = self.persist_locked(&guard).await;
    }

    pub async fn count_open_sessions_for_initiator(&self, sender: &str) -> usize {
        let guard = self.sessions.read().await;
        guard
            .values()
            .filter(|session| {
                session.initiator_sender == sender
                    && session.state == crate::session::SessionState::Open
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, SessionState};
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_session(id: &str) -> Session {
        Session {
            session_id: id.into(),
            state: SessionState::Open,
            ttl_expiry: 10,
            started_at_unix_ms: 1,
            resolution: None,
            mode: "macp.mode.decision.v1".into(),
            mode_state: vec![1, 2, 3],
            participants: vec!["alice".into()],
            seen_message_ids: HashSet::from(["m1".into()]),
            intent: "intent".into(),
            mode_version: "1.0.0".into(),
            configuration_version: "cfg".into(),
            policy_version: "pol".into(),
            context: vec![9],
            roots: vec![crate::pb::Root {
                uri: "root://1".into(),
                name: "r1".into(),
            }],
            initiator_sender: "alice".into(),
        }
    }

    #[tokio::test]
    async fn persistent_registry_round_trip() {
        let base = std::env::temp_dir().join(format!(
            "macp-registry-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let registry = SessionRegistry::with_persistence(&base).unwrap();
        registry
            .insert_session_for_test("s1".into(), sample_session("s1"))
            .await;

        let reopened = SessionRegistry::with_persistence(&base).unwrap();
        let session = reopened.get_session("s1").await.unwrap();
        assert_eq!(session.mode, "macp.mode.decision.v1");
        assert_eq!(session.mode_version, "1.0.0");
        assert!(session.seen_message_ids.contains("m1"));
    }
}
