use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::MacpError;
use crate::log_store::{EntryKind, LogEntry, LogStore};
use crate::mode::decision::DecisionMode;
use crate::mode::multi_round::MultiRoundMode;
use crate::mode::{Mode, ModeResponse};
use crate::pb::Envelope;
use crate::registry::SessionRegistry;
use crate::session::{parse_session_start_ttl_ms, Session, SessionState};

pub struct Runtime {
    pub registry: Arc<SessionRegistry>,
    pub log_store: Arc<LogStore>,
    modes: HashMap<String, Box<dyn Mode>>,
}

impl Runtime {
    pub fn new(registry: Arc<SessionRegistry>, log_store: Arc<LogStore>) -> Self {
        let mut modes: HashMap<String, Box<dyn Mode>> = HashMap::new();
        modes.insert("decision".into(), Box::new(DecisionMode));
        modes.insert("multi_round".into(), Box::new(MultiRoundMode));

        Self {
            registry,
            log_store,
            modes,
        }
    }

    fn resolve_mode_name(mode_field: &str) -> &str {
        if mode_field.is_empty() {
            "decision"
        } else {
            mode_field
        }
    }

    fn make_incoming_entry(env: &Envelope) -> LogEntry {
        LogEntry {
            message_id: env.message_id.clone(),
            received_at_ms: Utc::now().timestamp_millis(),
            sender: env.sender.clone(),
            message_type: env.message_type.clone(),
            raw_payload: env.payload.clone(),
            entry_kind: EntryKind::Incoming,
        }
    }

    fn make_internal_entry(message_type: &str, payload: &[u8]) -> LogEntry {
        LogEntry {
            message_id: String::new(),
            received_at_ms: Utc::now().timestamp_millis(),
            sender: "_runtime".into(),
            message_type: message_type.into(),
            raw_payload: payload.to_vec(),
            entry_kind: EntryKind::Internal,
        }
    }

    fn apply_mode_response(session: &mut Session, response: ModeResponse) {
        match response {
            ModeResponse::NoOp => {}
            ModeResponse::PersistState(s) => {
                session.mode_state = s;
            }
            ModeResponse::Resolve(r) => {
                session.state = SessionState::Resolved;
                session.resolution = Some(r);
            }
            ModeResponse::PersistAndResolve { state, resolution } => {
                session.mode_state = state;
                session.state = SessionState::Resolved;
                session.resolution = Some(resolution);
            }
        }
    }

    pub async fn process(&self, env: &Envelope) -> Result<(), MacpError> {
        if env.message_type == "SessionStart" {
            self.process_session_start(env).await
        } else {
            self.process_message(env).await
        }
    }

    async fn process_session_start(&self, env: &Envelope) -> Result<(), MacpError> {
        let mode_name = Self::resolve_mode_name(&env.mode);
        let mode = self.modes.get(mode_name).ok_or(MacpError::UnknownMode)?;

        let ttl_ms = parse_session_start_ttl_ms(&env.payload)?;

        let mut guard = self.registry.sessions.write().await;

        if guard.contains_key(&env.session_id) {
            return Err(MacpError::DuplicateSession);
        }

        let ttl_expiry = Utc::now().timestamp_millis() + ttl_ms;

        // Create session log and append incoming entry
        self.log_store.create_session_log(&env.session_id).await;
        self.log_store
            .append(&env.session_id, Self::make_incoming_entry(env))
            .await;

        // Create session with initial state
        let session = Session {
            session_id: env.session_id.clone(),
            state: SessionState::Open,
            ttl_expiry,
            resolution: None,
            mode: mode_name.to_string(),
            mode_state: vec![],
            participants: vec![],
        };

        // Call mode's on_session_start
        let response = mode.on_session_start(&session, env)?;

        // Insert session and apply response
        let mut session = session;
        Self::apply_mode_response(&mut session, response);

        // Extract participants from mode_state for multi_round
        if mode_name == "multi_round" && !session.mode_state.is_empty() {
            if let Ok(state) = serde_json::from_slice::<crate::mode::multi_round::MultiRoundState>(
                &session.mode_state,
            ) {
                session.participants = state.participants.clone();
            }
        }

        guard.insert(env.session_id.clone(), session);

        Ok(())
    }

    async fn process_message(&self, env: &Envelope) -> Result<(), MacpError> {
        let mut guard = self.registry.sessions.write().await;

        let session = guard
            .get_mut(&env.session_id)
            .ok_or(MacpError::UnknownSession)?;

        // TTL check
        let now = Utc::now().timestamp_millis();
        if session.state == SessionState::Open && now > session.ttl_expiry {
            // Log internal TTL expiry event before state mutation
            self.log_store
                .append(
                    &env.session_id,
                    Self::make_internal_entry("TtlExpired", b""),
                )
                .await;
            session.state = SessionState::Expired;
            return Err(MacpError::TtlExpired);
        }

        if session.state != SessionState::Open {
            return Err(MacpError::SessionNotOpen);
        }

        // Log incoming message before mode dispatch
        self.log_store
            .append(&env.session_id, Self::make_incoming_entry(env))
            .await;

        let mode_name = session.mode.clone();
        let mode = self.modes.get(&mode_name).ok_or(MacpError::UnknownMode)?;

        let response = mode.on_message(session, env)?;
        Self::apply_mode_response(session, response);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_runtime() -> Runtime {
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        Runtime::new(registry, log_store)
    }

    fn env(
        mode: &str,
        message_type: &str,
        message_id: &str,
        session_id: &str,
        sender: &str,
        payload: &[u8],
    ) -> Envelope {
        Envelope {
            macp_version: "v1".into(),
            mode: mode.into(),
            message_type: message_type.into(),
            message_id: message_id.into(),
            session_id: session_id.into(),
            sender: sender.into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: payload.to_vec(),
        }
    }

    #[tokio::test]
    async fn decision_mode_full_flow() {
        let rt = make_runtime();

        // SessionStart
        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        // Normal message
        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        rt.process(&e).await.unwrap();

        // Resolve
        let e = env("decision", "Message", "m3", "s1", "alice", b"resolve");
        rt.process(&e).await.unwrap();

        // After resolve
        let e = env("decision", "Message", "m4", "s1", "alice", b"nope");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }

    #[tokio::test]
    async fn empty_mode_defaults_to_decision() {
        let rt = make_runtime();

        let e = env("", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let guard = rt.registry.sessions.read().await;
        assert_eq!(guard["s1"].mode, "decision");
    }

    #[tokio::test]
    async fn unknown_mode_rejected() {
        let rt = make_runtime();

        let e = env("nonexistent", "SessionStart", "m1", "s1", "alice", b"");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "UnknownMode");
    }

    #[tokio::test]
    async fn multi_round_flow() {
        let rt = make_runtime();

        let payload = r#"{"participants":["alice","bob"],"convergence":{"type":"all_equal"}}"#;
        let e = env(
            "multi_round",
            "SessionStart",
            "m0",
            "s1",
            "creator",
            payload.as_bytes(),
        );
        rt.process(&e).await.unwrap();

        // Alice contributes
        let e = env(
            "multi_round",
            "Contribute",
            "m1",
            "s1",
            "alice",
            br#"{"value":"option_a"}"#,
        );
        rt.process(&e).await.unwrap();

        // Bob contributes different value — no convergence
        let e = env(
            "multi_round",
            "Contribute",
            "m2",
            "s1",
            "bob",
            br#"{"value":"option_b"}"#,
        );
        rt.process(&e).await.unwrap();

        {
            let guard = rt.registry.sessions.read().await;
            assert_eq!(guard["s1"].state, SessionState::Open);
        }

        // Bob revises to match alice — convergence
        let e = env(
            "multi_round",
            "Contribute",
            "m3",
            "s1",
            "bob",
            br#"{"value":"option_a"}"#,
        );
        rt.process(&e).await.unwrap();

        {
            let guard = rt.registry.sessions.read().await;
            assert_eq!(guard["s1"].state, SessionState::Resolved);
            let resolution = guard["s1"].resolution.as_ref().unwrap();
            let res: serde_json::Value = serde_json::from_slice(resolution).unwrap();
            assert_eq!(res["converged_value"], "option_a");
        }
    }

    #[tokio::test]
    async fn mode_response_apply_noop() {
        let mut session = Session {
            session_id: "s".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            resolution: None,
            mode: "decision".into(),
            mode_state: vec![],
            participants: vec![],
        };
        Runtime::apply_mode_response(&mut session, ModeResponse::NoOp);
        assert_eq!(session.state, SessionState::Open);
        assert!(session.resolution.is_none());
    }

    #[tokio::test]
    async fn mode_response_apply_persist_and_resolve() {
        let mut session = Session {
            session_id: "s".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            resolution: None,
            mode: "multi_round".into(),
            mode_state: vec![],
            participants: vec![],
        };
        Runtime::apply_mode_response(
            &mut session,
            ModeResponse::PersistAndResolve {
                state: b"new_state".to_vec(),
                resolution: b"resolved_data".to_vec(),
            },
        );
        assert_eq!(session.state, SessionState::Resolved);
        assert_eq!(session.mode_state, b"new_state");
        assert_eq!(session.resolution, Some(b"resolved_data".to_vec()));
    }

    #[tokio::test]
    async fn log_before_mutate_ordering() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let log = rt.log_store.get_log("s1").await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].message_type, "SessionStart");
        assert_eq!(log[0].entry_kind, EntryKind::Incoming);
    }

    #[tokio::test]
    async fn ttl_expiry_logs_internal_entry() {
        let rt = make_runtime();

        // Create session with very short TTL
        let e = env(
            "decision",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            br#"{"ttl_ms":1}"#,
        );
        rt.process(&e).await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "TtlExpired");

        let log = rt.log_store.get_log("s1").await.unwrap();
        // Should have: SessionStart (incoming) + TtlExpired (internal)
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].entry_kind, EntryKind::Internal);
        assert_eq!(log[1].message_type, "TtlExpired");
    }
}
