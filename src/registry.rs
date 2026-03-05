use crate::session::Session;
use std::collections::HashMap;
use tokio::sync::RwLock;

pub struct SessionRegistry {
    pub(crate) sessions: RwLock<HashMap<String, Session>>,
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
        }
    }

    /// Get a clone of a session by ID. Returns None if not found.
    pub async fn get_session(&self, session_id: &str) -> Option<Session> {
        let guard = self.sessions.read().await;
        guard.get(session_id).cloned()
    }

    /// Insert a session directly. Used by tests and for pre-populating state.
    pub async fn insert_session_for_test(&self, session_id: String, session: Session) {
        let mut guard = self.sessions.write().await;
        guard.insert(session_id, session);
    }
}
