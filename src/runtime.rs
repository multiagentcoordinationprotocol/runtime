use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::error::MacpError;
use crate::log_store::{EntryKind, LogEntry, LogStore};
use crate::mode::decision::DecisionMode;
use crate::mode::handoff::HandoffMode;
use crate::mode::multi_round::MultiRoundMode;
use crate::mode::proposal::ProposalMode;
use crate::mode::quorum::QuorumMode;
use crate::mode::task::TaskMode;
use crate::mode::{standard_mode_names, Mode, ModeResponse};
use crate::pb::Envelope;
use crate::registry::SessionRegistry;
use crate::session::{extract_ttl_ms, parse_session_start_payload, Session, SessionState};

/// Result of processing a message through the runtime.
#[derive(Debug)]
pub struct ProcessResult {
    pub session_state: SessionState,
    pub duplicate: bool,
}

pub struct Runtime {
    pub registry: Arc<SessionRegistry>,
    pub log_store: Arc<LogStore>,
    modes: HashMap<String, Box<dyn Mode>>,
}

impl Runtime {
    pub fn new(registry: Arc<SessionRegistry>, log_store: Arc<LogStore>) -> Self {
        let mut modes: HashMap<String, Box<dyn Mode>> = HashMap::new();

        // Standards-track canonical mode names
        modes.insert("macp.mode.decision.v1".into(), Box::new(DecisionMode));
        modes.insert("macp.mode.proposal.v1".into(), Box::new(ProposalMode));
        modes.insert("macp.mode.task.v1".into(), Box::new(TaskMode));
        modes.insert("macp.mode.handoff.v1".into(), Box::new(HandoffMode));
        modes.insert("macp.mode.quorum.v1".into(), Box::new(QuorumMode));

        // Experimental mode (not advertised via ListModes but functional)
        modes.insert("macp.mode.multi_round.v1".into(), Box::new(MultiRoundMode));

        // Short aliases for backward compatibility (legacy only for decision/multi_round)
        modes.insert("decision".into(), Box::new(DecisionMode));
        modes.insert("multi_round".into(), Box::new(MultiRoundMode));

        Self {
            registry,
            log_store,
            modes,
        }
    }

    /// Returns the standards-track mode names in canonical registry order.
    pub fn registered_mode_names(&self) -> Vec<String> {
        standard_mode_names()
            .iter()
            .filter(|mode_name| self.modes.contains_key(**mode_name))
            .map(|mode_name| (*mode_name).to_string())
            .collect()
    }

    fn resolve_mode_name(mode_field: &str) -> &str {
        if mode_field.is_empty() {
            "macp.mode.decision.v1"
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

    pub async fn process(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
        match env.message_type.as_str() {
            "SessionStart" => self.process_session_start(env).await,
            "Signal" => self.process_signal(env).await,
            _ => self.process_message(env).await,
        }
    }

    async fn process_session_start(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
        let mode_name = Self::resolve_mode_name(&env.mode);
        let mode = self.modes.get(mode_name).ok_or(MacpError::UnknownMode)?;

        // Parse protobuf SessionStartPayload
        let start_payload = parse_session_start_payload(&env.payload)?;
        let ttl_ms = extract_ttl_ms(&start_payload)?;

        let mut guard = self.registry.sessions.write().await;

        // Check for duplicate session — idempotent if same message_id
        if let Some(existing) = guard.get(&env.session_id) {
            if existing.seen_message_ids.contains(&env.message_id) {
                return Ok(ProcessResult {
                    session_state: existing.state.clone(),
                    duplicate: true,
                });
            }
            return Err(MacpError::DuplicateSession);
        }

        let now = Utc::now().timestamp_millis();
        let ttl_expiry = now + ttl_ms;

        // Extract participants from SessionStartPayload
        let participants = start_payload.participants.clone();

        // Create session with initial state and RFC version fields
        let session = Session {
            session_id: env.session_id.clone(),
            state: SessionState::Open,
            ttl_expiry,
            started_at_unix_ms: now,
            resolution: None,
            mode: mode_name.to_string(),
            mode_state: vec![],
            participants,
            seen_message_ids: HashSet::new(),
            intent: start_payload.intent.clone(),
            mode_version: start_payload.mode_version.clone(),
            configuration_version: start_payload.configuration_version.clone(),
            policy_version: start_payload.policy_version.clone(),
            context: start_payload.context.clone(),
            roots: start_payload.roots.clone(),
            initiator_sender: env.sender.clone(),
        };

        // Call mode's on_session_start BEFORE recording side effects
        let response = mode.on_session_start(&session, env)?;

        // Only on success: create log and record message_id
        self.log_store.create_session_log(&env.session_id).await;
        self.log_store
            .append(&env.session_id, Self::make_incoming_entry(env))
            .await;

        let mut session = session;
        session.seen_message_ids.insert(env.message_id.clone());
        Self::apply_mode_response(&mut session, response);

        // For multi_round mode, extract participants from mode_state if not already set
        if session.participants.is_empty() && !session.mode_state.is_empty() {
            if let Ok(state) = serde_json::from_slice::<crate::mode::multi_round::MultiRoundState>(
                &session.mode_state,
            ) {
                session.participants = state.participants.clone();
            }
        }

        let result_state = session.state.clone();
        guard.insert(env.session_id.clone(), session);

        Ok(ProcessResult {
            session_state: result_state,
            duplicate: false,
        })
    }

    async fn process_message(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
        let mut guard = self.registry.sessions.write().await;

        let session = guard
            .get_mut(&env.session_id)
            .ok_or(MacpError::UnknownSession)?;

        // Message deduplication
        if session.seen_message_ids.contains(&env.message_id) {
            return Ok(ProcessResult {
                session_state: session.state.clone(),
                duplicate: true,
            });
        }

        // TTL check
        let now = Utc::now().timestamp_millis();
        if session.state == SessionState::Open && now > session.ttl_expiry {
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

        let mode_name = session.mode.clone();
        let mode = self.modes.get(&mode_name).ok_or(MacpError::UnknownMode)?;

        // Mode-aware authorization (replaces hardcoded participant check)
        mode.authorize_sender(session, env)?;

        // Dispatch to mode BEFORE recording side effects
        let response = mode.on_message(session, env)?;

        // Only on success: record message_id and log
        session.seen_message_ids.insert(env.message_id.clone());
        self.log_store
            .append(&env.session_id, Self::make_incoming_entry(env))
            .await;

        Self::apply_mode_response(session, response);

        Ok(ProcessResult {
            session_state: session.state.clone(),
            duplicate: false,
        })
    }

    /// Process a Signal message. Signals are non-binding and non-session-scoped.
    async fn process_signal(&self, _env: &Envelope) -> Result<ProcessResult, MacpError> {
        Ok(ProcessResult {
            session_state: SessionState::Open,
            duplicate: false,
        })
    }

    /// Get a session with TTL check. Transitions expired sessions to Expired state.
    pub async fn get_session_checked(&self, session_id: &str) -> Option<Session> {
        let mut guard = self.registry.sessions.write().await;
        if let Some(session) = guard.get_mut(session_id) {
            let now = Utc::now().timestamp_millis();
            if session.state == SessionState::Open && now > session.ttl_expiry {
                self.log_store
                    .append(session_id, Self::make_internal_entry("TtlExpired", b""))
                    .await;
                session.state = SessionState::Expired;
            }
            Some(session.clone())
        } else {
            None
        }
    }

    /// Cancel a session by ID. Idempotent for already-resolved/expired sessions.
    pub async fn cancel_session(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<ProcessResult, MacpError> {
        let mut guard = self.registry.sessions.write().await;

        let session = guard.get_mut(session_id).ok_or(MacpError::UnknownSession)?;

        // TTL check: transition expired sessions
        let now = Utc::now().timestamp_millis();
        if session.state == SessionState::Open && now > session.ttl_expiry {
            self.log_store
                .append(session_id, Self::make_internal_entry("TtlExpired", b""))
                .await;
            session.state = SessionState::Expired;
        }

        // Idempotent: already resolved or expired
        if session.state == SessionState::Resolved || session.state == SessionState::Expired {
            return Ok(ProcessResult {
                session_state: session.state.clone(),
                duplicate: false,
            });
        }

        // Log cancellation
        self.log_store
            .append(
                session_id,
                Self::make_internal_entry("SessionCancel", reason.as_bytes()),
            )
            .await;

        session.state = SessionState::Expired;

        Ok(ProcessResult {
            session_state: SessionState::Expired,
            duplicate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision_pb::ProposalPayload;
    use crate::handoff_pb as hpb;
    use crate::pb::CommitmentPayload;
    use crate::pb::SessionStartPayload;
    use crate::proposal_pb as ppb;
    use crate::quorum_pb as qpb;
    use crate::task_pb as tpb;
    use prost::Message;

    fn make_runtime() -> Runtime {
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        Runtime::new(registry, log_store)
    }

    fn encode_session_start(ttl_ms: i64, participants: Vec<String>) -> Vec<u8> {
        let payload = SessionStartPayload {
            intent: String::new(),
            participants,
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            ttl_ms,
            context: vec![],
            roots: vec![],
        };
        payload.encode_to_vec()
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
            macp_version: "1.0".into(),
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

        // Empty mode defaults to macp.mode.decision.v1 which requires participants
        let payload = encode_session_start(0, vec!["alice".into()]);
        let e = env("", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        let guard = rt.registry.sessions.read().await;
        assert_eq!(guard["s1"].mode, "macp.mode.decision.v1");
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

        let start_payload = encode_session_start(
            0, // default TTL
            vec!["alice".into(), "bob".into()],
        );
        let e = env(
            "multi_round",
            "SessionStart",
            "m0",
            "s1",
            "creator",
            &start_payload,
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
            started_at_unix_ms: 0,
            resolution: None,
            mode: "decision".into(),
            mode_state: vec![],
            participants: vec![],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: vec![],
            roots: vec![],
            initiator_sender: String::new(),
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
            started_at_unix_ms: 0,
            resolution: None,
            mode: "multi_round".into(),
            mode_state: vec![],
            participants: vec![],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: vec![],
            roots: vec![],
            initiator_sender: String::new(),
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

        let payload = encode_session_start(1, vec![]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "TtlExpired");

        let log = rt.log_store.get_log("s1").await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].entry_kind, EntryKind::Internal);
        assert_eq!(log[1].message_type, "TtlExpired");
    }

    // --- Phase 3: Deduplication tests ---

    #[tokio::test]
    async fn duplicate_message_returns_duplicate_true() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let result = rt.process(&e).await.unwrap();
        assert!(!result.duplicate);

        // Same message_id again
        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let result = rt.process(&e).await.unwrap();
        assert!(result.duplicate);
    }

    #[tokio::test]
    async fn duplicate_session_start_same_message_id_is_idempotent() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        // Same session_id and same message_id
        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        let result = rt.process(&e).await.unwrap();
        assert!(result.duplicate);
    }

    #[tokio::test]
    async fn duplicate_session_start_different_message_id_errors() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        // Same session_id but different message_id
        let e = env("decision", "SessionStart", "m2", "s1", "alice", b"");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "DuplicateSession");
    }

    #[tokio::test]
    async fn duplicate_does_not_log() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        rt.process(&e).await.unwrap();

        let log_before = rt.log_store.get_log("s1").await.unwrap().len();

        // Duplicate
        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let result = rt.process(&e).await.unwrap();
        assert!(result.duplicate);

        let log_after = rt.log_store.get_log("s1").await.unwrap().len();
        assert_eq!(log_before, log_after); // No new log entry
    }

    // --- CancelSession tests ---

    #[tokio::test]
    async fn cancel_open_session() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let result = rt.cancel_session("s1", "test cancel").await.unwrap();
        assert_eq!(result.session_state, SessionState::Expired);

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.state, SessionState::Expired);
    }

    #[tokio::test]
    async fn cancel_resolved_session_is_idempotent() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"resolve");
        rt.process(&e).await.unwrap();

        let result = rt.cancel_session("s1", "too late").await.unwrap();
        assert_eq!(result.session_state, SessionState::Resolved);
    }

    #[tokio::test]
    async fn cancel_unknown_session_errors() {
        let rt = make_runtime();
        let err = rt
            .cancel_session("nonexistent", "reason")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "UnknownSession");
    }

    #[tokio::test]
    async fn message_after_cancel_rejected() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        rt.cancel_session("s1", "cancelled").await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }

    #[tokio::test]
    async fn cancel_logs_entry() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        rt.cancel_session("s1", "test reason").await.unwrap();

        let log = rt.log_store.get_log("s1").await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].message_type, "SessionCancel");
        assert_eq!(log[1].entry_kind, EntryKind::Internal);
    }

    // --- Participant validation tests ---

    #[tokio::test]
    async fn forbidden_when_sender_not_in_participants() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        // "charlie" is not a participant
        let e = env("decision", "Message", "m2", "s1", "charlie", b"hello");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");
    }

    #[tokio::test]
    async fn allowed_when_participants_empty() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        // Any sender allowed when participants is empty
        let e = env("decision", "Message", "m2", "s1", "charlie", b"hello");
        rt.process(&e).await.unwrap();
    }

    #[tokio::test]
    async fn allowed_when_sender_is_participant() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        rt.process(&e).await.unwrap();
    }

    // --- Mode naming tests ---

    #[tokio::test]
    async fn rfc_mode_name_works() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into()]);
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            &payload,
        );
        rt.process(&e).await.unwrap();

        let guard = rt.registry.sessions.read().await;
        assert_eq!(guard["s1"].mode, "macp.mode.decision.v1");
    }

    #[tokio::test]
    async fn short_alias_still_works() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let guard = rt.registry.sessions.read().await;
        assert_eq!(guard["s1"].mode, "decision");
    }

    // --- Signal tests ---

    #[tokio::test]
    async fn signal_with_empty_session_id_accepted() {
        let rt = make_runtime();

        let e = env("", "Signal", "sig1", "", "alice", b"");
        let result = rt.process(&e).await.unwrap();
        assert!(!result.duplicate);
    }

    #[tokio::test]
    async fn signal_does_not_create_session() {
        let rt = make_runtime();

        let e = env("", "Signal", "sig1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await;
        assert!(s.is_none());
    }

    // --- ProcessResult state field tests ---

    #[tokio::test]
    async fn process_result_state_open_after_session_start() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        let result = rt.process(&e).await.unwrap();
        assert_eq!(result.session_state, SessionState::Open);
        assert!(!result.duplicate);
    }

    #[tokio::test]
    async fn process_result_state_resolved_after_resolve() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"resolve");
        let result = rt.process(&e).await.unwrap();
        assert_eq!(result.session_state, SessionState::Resolved);
    }

    #[tokio::test]
    async fn process_result_state_open_for_normal_message() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let result = rt.process(&e).await.unwrap();
        assert_eq!(result.session_state, SessionState::Open);
    }

    // --- started_at_unix_ms tests ---

    #[tokio::test]
    async fn started_at_unix_ms_populated() {
        let rt = make_runtime();
        let before = chrono::Utc::now().timestamp_millis();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let after = chrono::Utc::now().timestamp_millis();
        let s = rt.registry.get_session("s1").await.unwrap();
        assert!(s.started_at_unix_ms >= before);
        assert!(s.started_at_unix_ms <= after);
    }

    // --- Dedup across different sessions ---

    #[tokio::test]
    async fn same_message_id_different_sessions_not_duplicate() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "SessionStart", "m1", "s2", "alice", b"");
        let result = rt.process(&e).await.unwrap();
        assert!(!result.duplicate);
    }

    // --- Cancel already-expired session is idempotent ---

    #[tokio::test]
    async fn cancel_already_expired_session_is_idempotent() {
        let rt = make_runtime();

        let payload = encode_session_start(1, vec![]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        // Wait for TTL to expire, then trigger expiry
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let _ = rt.process(&e).await; // triggers expiry

        // Cancel expired session — should be idempotent
        let result = rt.cancel_session("s1", "already expired").await.unwrap();
        assert_eq!(result.session_state, SessionState::Expired);
    }

    // --- Multi-round with protobuf participants ---

    #[tokio::test]
    async fn multi_round_participants_from_protobuf_payload() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env(
            "multi_round",
            "SessionStart",
            "m0",
            "s1",
            "creator",
            &payload,
        );
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.participants, vec!["alice", "bob"]);
    }

    #[tokio::test]
    async fn multi_round_with_participant_validation() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env(
            "multi_round",
            "SessionStart",
            "m0",
            "s1",
            "creator",
            &payload,
        );
        rt.process(&e).await.unwrap();

        // Unauthorized participant
        let e = env(
            "multi_round",
            "Contribute",
            "m1",
            "s1",
            "charlie",
            br#"{"value":"x"}"#,
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");

        // Authorized participant
        let e = env(
            "multi_round",
            "Contribute",
            "m2",
            "s1",
            "alice",
            br#"{"value":"x"}"#,
        );
        rt.process(&e).await.unwrap();
    }

    // --- RFC mode names for multi_round ---

    #[tokio::test]
    async fn rfc_mode_name_multi_round_works() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into()]);
        let e = env(
            "macp.mode.multi_round.v1",
            "SessionStart",
            "m0",
            "s1",
            "alice",
            &payload,
        );
        rt.process(&e).await.unwrap();

        let guard = rt.registry.sessions.read().await;
        assert_eq!(guard["s1"].mode, "macp.mode.multi_round.v1");
    }

    // --- Signal with payload ---

    #[tokio::test]
    async fn signal_with_payload_accepted() {
        let rt = make_runtime();

        let e = env("", "Signal", "sig1", "", "alice", b"some signal data");
        let result = rt.process(&e).await.unwrap();
        assert_eq!(result.session_state, SessionState::Open);
    }

    // --- ModeResponse::PersistState ---

    #[tokio::test]
    async fn mode_response_apply_persist_state() {
        let mut session = Session {
            session_id: "s".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            started_at_unix_ms: 0,
            resolution: None,
            mode: "decision".into(),
            mode_state: vec![],
            participants: vec![],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: vec![],
            roots: vec![],
            initiator_sender: String::new(),
        };
        Runtime::apply_mode_response(
            &mut session,
            ModeResponse::PersistState(b"persisted".to_vec()),
        );
        assert_eq!(session.state, SessionState::Open);
        assert_eq!(session.mode_state, b"persisted");
        assert!(session.resolution.is_none());
    }

    #[tokio::test]
    async fn mode_response_apply_resolve() {
        let mut session = Session {
            session_id: "s".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            started_at_unix_ms: 0,
            resolution: None,
            mode: "decision".into(),
            mode_state: vec![],
            participants: vec![],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: vec![],
            roots: vec![],
            initiator_sender: String::new(),
        };
        Runtime::apply_mode_response(&mut session, ModeResponse::Resolve(b"resolved".to_vec()));
        assert_eq!(session.state, SessionState::Resolved);
        assert_eq!(session.resolution, Some(b"resolved".to_vec()));
        assert!(session.mode_state.is_empty());
    }

    // --- Multiple sessions isolation ---

    #[tokio::test]
    async fn multiple_sessions_independent() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "SessionStart", "m2", "s2", "bob", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m3", "s1", "alice", b"resolve");
        rt.process(&e).await.unwrap();

        let s2 = rt.registry.get_session("s2").await.unwrap();
        assert_eq!(s2.state, SessionState::Open);

        let s1 = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s1.state, SessionState::Resolved);
    }

    // --- Protobuf TTL from SessionStartPayload ---

    #[tokio::test]
    async fn session_start_with_protobuf_ttl() {
        let rt = make_runtime();

        let payload = encode_session_start(30_000, vec![]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        assert!(s.ttl_expiry > now);
        assert!(s.ttl_expiry <= now + 31_000);
    }

    #[tokio::test]
    async fn session_start_invalid_protobuf_ttl_rejected() {
        let rt = make_runtime();

        let payload = encode_session_start(-100, vec![]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    // --- Cancel preserves log integrity ---

    #[tokio::test]
    async fn cancel_reason_recorded_in_log() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        rt.cancel_session("s1", "user requested").await.unwrap();

        let log = rt.log_store.get_log("s1").await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].message_type, "SessionCancel");
        assert_eq!(log[1].raw_payload, b"user requested");
        assert_eq!(log[1].sender, "_runtime");
    }

    // --- Dedup does not invoke mode ---

    #[tokio::test]
    async fn duplicate_resolve_does_not_double_resolve() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"resolve");
        let result = rt.process(&e).await.unwrap();
        assert_eq!(result.session_state, SessionState::Resolved);
        assert!(!result.duplicate);

        let e = env("decision", "Message", "m2", "s1", "alice", b"resolve");
        let result = rt.process(&e).await.unwrap();
        assert!(result.duplicate);
        assert_eq!(result.session_state, SessionState::Resolved);
    }

    // --- Version fields stored on session ---

    #[tokio::test]
    async fn session_stores_version_fields() {
        let rt = make_runtime();

        let payload = SessionStartPayload {
            intent: "test coordination".into(),
            participants: vec![],
            mode_version: "1.0".into(),
            configuration_version: "cfg-v2".into(),
            policy_version: "pol-v1".into(),
            ttl_ms: 0,
            context: vec![],
            roots: vec![],
        };
        let e = env(
            "decision",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            &payload.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.intent, "test coordination");
        assert_eq!(s.mode_version, "1.0");
        assert_eq!(s.configuration_version, "cfg-v2");
        assert_eq!(s.policy_version, "pol-v1");
    }

    // --- PR #1a: Admission pipeline bug tests ---

    #[tokio::test]
    async fn rejected_message_does_not_burn_message_id() {
        let rt = make_runtime();

        // Create a decision session with a proposal so we can test invalid payloads
        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        // Send a Proposal with invalid payload — should be rejected
        let e = env("decision", "Proposal", "m2", "s1", "alice", b"not protobuf");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");

        // Retry the same message_id with valid payload — should succeed (not duplicate)
        let valid_payload = ProposalPayload {
            proposal_id: "p1".into(),
            option: "option_a".into(),
            rationale: "test".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let e = env("decision", "Proposal", "m2", "s1", "alice", &valid_payload);
        let result = rt.process(&e).await.unwrap();
        assert!(!result.duplicate);
    }

    #[tokio::test]
    async fn rejected_message_not_in_log() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let log_before = rt.log_store.get_log("s1").await.unwrap().len();

        // Send a Proposal with invalid payload — should be rejected
        let e = env("decision", "Proposal", "m2", "s1", "alice", b"not protobuf");
        let _ = rt.process(&e).await;

        let log_after = rt.log_store.get_log("s1").await.unwrap().len();
        assert_eq!(log_before, log_after); // No new log entry for rejected message
    }

    #[tokio::test]
    async fn accepted_messages_still_dedup_correctly() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let result = rt.process(&e).await.unwrap();
        assert!(!result.duplicate);

        // Same message_id again — should be duplicate
        let e = env("decision", "Message", "m2", "s1", "alice", b"hello");
        let result = rt.process(&e).await.unwrap();
        assert!(result.duplicate);
    }

    #[tokio::test]
    async fn session_start_mode_rejection_no_side_effects() {
        let rt = make_runtime();

        // MultiRound requires participants — empty participants should fail
        let e = env("multi_round", "SessionStart", "m1", "s1", "creator", b"");
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");

        // Session should not exist
        assert!(rt.registry.get_session("s1").await.is_none());
        // Log should not exist
        assert!(rt.log_store.get_log("s1").await.is_none());
    }

    // --- PR #1b: TTL on GetSession/CancelSession tests ---

    #[tokio::test]
    async fn get_session_checked_transitions_expired() {
        let rt = make_runtime();

        let payload = encode_session_start(1, vec![]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let session = rt.get_session_checked("s1").await.unwrap();
        assert_eq!(session.state, SessionState::Expired);
    }

    #[tokio::test]
    async fn cancel_expired_session_returns_expired_idempotent() {
        let rt = make_runtime();

        let payload = encode_session_start(1, vec![]);
        let e = env("decision", "SessionStart", "m1", "s1", "alice", &payload);
        rt.process(&e).await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Cancel should detect TTL expiry and return Expired idempotently
        let result = rt.cancel_session("s1", "cancel attempt").await.unwrap();
        assert_eq!(result.session_state, SessionState::Expired);
    }

    // --- PR #5: Participant enforcement tests ---

    #[tokio::test]
    async fn canonical_decision_mode_requires_participants() {
        let rt = make_runtime();

        // macp.mode.decision.v1 with no participants should fail
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            b"",
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[tokio::test]
    async fn canonical_decision_mode_with_participants_succeeds() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            &payload,
        );
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.mode, "macp.mode.decision.v1");
        assert_eq!(s.participants, vec!["alice", "bob"]);
    }

    #[tokio::test]
    async fn legacy_decision_alias_allows_empty_participants() {
        let rt = make_runtime();

        let e = env("decision", "SessionStart", "m1", "s1", "alice", b"");
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.mode, "decision");
    }

    // --- Phase 2: Session data model tests ---

    #[tokio::test]
    async fn session_stores_context_and_roots() {
        let rt = make_runtime();

        let payload = SessionStartPayload {
            intent: String::new(),
            participants: vec!["alice".into()],
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            ttl_ms: 0,
            context: b"some context".to_vec(),
            roots: vec![crate::pb::Root {
                uri: "file:///tmp".into(),
                name: "test-root".into(),
            }],
        };
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            &payload.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.context, b"some context");
        assert_eq!(s.roots.len(), 1);
        assert_eq!(s.roots[0].uri, "file:///tmp");
        assert_eq!(s.roots[0].name, "test-root");
    }

    #[tokio::test]
    async fn session_stores_initiator_sender() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into()]);
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "alice",
            &payload,
        );
        rt.process(&e).await.unwrap();

        let s = rt.registry.get_session("s1").await.unwrap();
        assert_eq!(s.initiator_sender, "alice");
    }

    // --- Phase 3: Mode-aware authorization tests ---

    #[tokio::test]
    async fn commitment_from_initiator_allowed_outside_participants() {
        let rt = make_runtime();

        // Create session with participants alice+bob, initiator is "coordinator"
        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m0",
            "s1",
            "coordinator",
            &payload,
        );
        rt.process(&e).await.unwrap();

        // Alice submits a proposal
        let proposal = crate::decision_pb::ProposalPayload {
            proposal_id: "p1".into(),
            option: "opt".into(),
            rationale: "r".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let e = env(
            "macp.mode.decision.v1",
            "Proposal",
            "m1",
            "s1",
            "alice",
            &proposal,
        );
        rt.process(&e).await.unwrap();

        // Coordinator (not in participants) sends Commitment — should succeed (no votes required per RFC)
        let e = env(
            "macp.mode.decision.v1",
            "Commitment",
            "m2",
            "s1",
            "coordinator",
            b"commit",
        );
        let result = rt.process(&e).await.unwrap();
        assert_eq!(result.session_state, SessionState::Resolved);
    }

    #[tokio::test]
    async fn proposal_from_non_participant_still_forbidden() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m0",
            "s1",
            "coordinator",
            &payload,
        );
        rt.process(&e).await.unwrap();

        // "charlie" not a participant and not sending Commitment
        let proposal = crate::decision_pb::ProposalPayload {
            proposal_id: "p1".into(),
            option: "opt".into(),
            rationale: "r".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let e = env(
            "macp.mode.decision.v1",
            "Proposal",
            "m1",
            "s1",
            "charlie",
            &proposal,
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");
    }

    #[tokio::test]
    async fn multi_round_authorization_unchanged() {
        let rt = make_runtime();

        let payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
        let e = env(
            "multi_round",
            "SessionStart",
            "m0",
            "s1",
            "creator",
            &payload,
        );
        rt.process(&e).await.unwrap();

        // "charlie" still forbidden (default authorize_sender)
        let e = env(
            "multi_round",
            "Contribute",
            "m1",
            "s1",
            "charlie",
            br#"{"value":"x"}"#,
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");

        // alice succeeds
        let e = env(
            "multi_round",
            "Contribute",
            "m2",
            "s1",
            "alice",
            br#"{"value":"x"}"#,
        );
        rt.process(&e).await.unwrap();
    }

    #[tokio::test]
    async fn proposal_mode_full_flow() {
        let rt = make_runtime();

        // SessionStart
        let start_payload = encode_session_start(0, vec!["buyer".into(), "seller".into()]);
        let e = env(
            "macp.mode.proposal.v1",
            "SessionStart",
            "m0",
            "s_prop",
            "buyer",
            &start_payload,
        );
        rt.process(&e).await.unwrap();

        // Proposal from seller
        let proposal = ppb::ProposalPayload {
            proposal_id: "p1".into(),
            title: "Offer".into(),
            summary: "$1200".into(),
            details: vec![],
            tags: vec![],
        };
        let e = env(
            "macp.mode.proposal.v1",
            "Proposal",
            "m1",
            "s_prop",
            "seller",
            &proposal.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Accept from buyer
        let accept = ppb::AcceptPayload {
            proposal_id: "p1".into(),
            reason: "agreed".into(),
        };
        let e = env(
            "macp.mode.proposal.v1",
            "Accept",
            "m2",
            "s_prop",
            "buyer",
            &accept.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Commitment from buyer (initiator)
        let commitment = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "proposal.accepted".into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy".into(),
            configuration_version: "config".into(),
        };
        let e = env(
            "macp.mode.proposal.v1",
            "Commitment",
            "m3",
            "s_prop",
            "buyer",
            &commitment.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Assert Resolved state
        {
            let guard = rt.registry.sessions.read().await;
            assert_eq!(guard["s_prop"].state, SessionState::Resolved);
        }

        // Post-resolution message should fail
        let e = env(
            "macp.mode.proposal.v1",
            "Proposal",
            "m4",
            "s_prop",
            "seller",
            b"nope",
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }

    #[tokio::test]
    async fn task_mode_full_flow() {
        let rt = make_runtime();

        // SessionStart
        let start_payload = encode_session_start(0, vec!["planner".into(), "worker".into()]);
        let e = env(
            "macp.mode.task.v1",
            "SessionStart",
            "m0",
            "s_task",
            "planner",
            &start_payload,
        );
        rt.process(&e).await.unwrap();

        // TaskRequest from planner
        let task_req = tpb::TaskRequestPayload {
            task_id: "t1".into(),
            title: "Build widget".into(),
            instructions: "Do it".into(),
            requested_assignee: "worker".into(),
            input: vec![],
            deadline_unix_ms: 0,
        };
        let e = env(
            "macp.mode.task.v1",
            "TaskRequest",
            "m1",
            "s_task",
            "planner",
            &task_req.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // TaskAccept from worker
        let task_accept = tpb::TaskAcceptPayload {
            task_id: "t1".into(),
            assignee: "worker".into(),
            reason: "ready".into(),
        };
        let e = env(
            "macp.mode.task.v1",
            "TaskAccept",
            "m2",
            "s_task",
            "worker",
            &task_accept.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // TaskComplete from worker
        let task_complete = tpb::TaskCompletePayload {
            task_id: "t1".into(),
            assignee: "worker".into(),
            output: b"result".to_vec(),
            summary: "done".into(),
        };
        let e = env(
            "macp.mode.task.v1",
            "TaskComplete",
            "m3",
            "s_task",
            "worker",
            &task_complete.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Commitment from planner (initiator)
        let commitment = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "task.completed".into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy".into(),
            configuration_version: "config".into(),
        };
        let e = env(
            "macp.mode.task.v1",
            "Commitment",
            "m4",
            "s_task",
            "planner",
            &commitment.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Assert Resolved state
        {
            let guard = rt.registry.sessions.read().await;
            assert_eq!(guard["s_task"].state, SessionState::Resolved);
        }

        // Post-resolution message should fail
        let e = env(
            "macp.mode.task.v1",
            "TaskRequest",
            "m5",
            "s_task",
            "planner",
            b"nope",
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }

    #[tokio::test]
    async fn handoff_mode_full_flow() {
        let rt = make_runtime();

        // SessionStart
        let start_payload = encode_session_start(0, vec!["owner".into(), "target".into()]);
        let e = env(
            "macp.mode.handoff.v1",
            "SessionStart",
            "m0",
            "s_hand",
            "owner",
            &start_payload,
        );
        rt.process(&e).await.unwrap();

        // HandoffOffer from owner
        let offer = hpb::HandoffOfferPayload {
            handoff_id: "h1".into(),
            target_participant: "target".into(),
            scope: "support".into(),
            reason: "escalate".into(),
        };
        let e = env(
            "macp.mode.handoff.v1",
            "HandoffOffer",
            "m1",
            "s_hand",
            "owner",
            &offer.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // HandoffAccept from target
        let accept = hpb::HandoffAcceptPayload {
            handoff_id: "h1".into(),
            accepted_by: "target".into(),
            reason: "ready".into(),
        };
        let e = env(
            "macp.mode.handoff.v1",
            "HandoffAccept",
            "m2",
            "s_hand",
            "target",
            &accept.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Commitment from owner (initiator)
        let commitment = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "handoff.accepted".into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy".into(),
            configuration_version: "config".into(),
        };
        let e = env(
            "macp.mode.handoff.v1",
            "Commitment",
            "m3",
            "s_hand",
            "owner",
            &commitment.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Assert Resolved state
        {
            let guard = rt.registry.sessions.read().await;
            assert_eq!(guard["s_hand"].state, SessionState::Resolved);
        }

        // Post-resolution message should fail
        let e = env(
            "macp.mode.handoff.v1",
            "HandoffOffer",
            "m4",
            "s_hand",
            "owner",
            b"nope",
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }

    #[tokio::test]
    async fn quorum_mode_full_flow() {
        let rt = make_runtime();

        // SessionStart
        let start_payload =
            encode_session_start(0, vec!["alice".into(), "bob".into(), "carol".into()]);
        let e = env(
            "macp.mode.quorum.v1",
            "SessionStart",
            "m0",
            "s_quorum",
            "coordinator",
            &start_payload,
        );
        rt.process(&e).await.unwrap();

        // ApprovalRequest from coordinator
        let approval_req = qpb::ApprovalRequestPayload {
            request_id: "r1".into(),
            action: "deploy".into(),
            summary: "Deploy v2".into(),
            details: vec![],
            required_approvals: 2,
        };
        let e = env(
            "macp.mode.quorum.v1",
            "ApprovalRequest",
            "m1",
            "s_quorum",
            "coordinator",
            &approval_req.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Approve from alice
        let approve_alice = qpb::ApprovePayload {
            request_id: "r1".into(),
            reason: "looks good".into(),
        };
        let e = env(
            "macp.mode.quorum.v1",
            "Approve",
            "m2",
            "s_quorum",
            "alice",
            &approve_alice.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Approve from bob
        let approve_bob = qpb::ApprovePayload {
            request_id: "r1".into(),
            reason: "ready".into(),
        };
        let e = env(
            "macp.mode.quorum.v1",
            "Approve",
            "m3",
            "s_quorum",
            "bob",
            &approve_bob.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Commitment from coordinator (initiator)
        let commitment = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "quorum.approved".into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy".into(),
            configuration_version: "config".into(),
        };
        let e = env(
            "macp.mode.quorum.v1",
            "Commitment",
            "m4",
            "s_quorum",
            "coordinator",
            &commitment.encode_to_vec(),
        );
        rt.process(&e).await.unwrap();

        // Assert Resolved state
        {
            let guard = rt.registry.sessions.read().await;
            assert_eq!(guard["s_quorum"].state, SessionState::Resolved);
        }

        // Post-resolution message should fail
        let e = env(
            "macp.mode.quorum.v1",
            "ApprovalRequest",
            "m5",
            "s_quorum",
            "coordinator",
            b"nope",
        );
        let err = rt.process(&e).await.unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }
}
