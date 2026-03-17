use chrono::Utc;
use std::collections::HashMap;
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
use crate::pb::{Envelope, SessionStartPayload};
use crate::registry::SessionRegistry;
use crate::session::{
    extract_ttl_ms, is_standard_mode, parse_session_start_payload,
    validate_standard_session_start_payload, Session, SessionState,
};

const EXPERIMENTAL_DEFAULT_TTL_MS: i64 = 60_000;

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
        modes.insert("macp.mode.decision.v1".into(), Box::new(DecisionMode));
        modes.insert("macp.mode.proposal.v1".into(), Box::new(ProposalMode));
        modes.insert("macp.mode.task.v1".into(), Box::new(TaskMode));
        modes.insert("macp.mode.handoff.v1".into(), Box::new(HandoffMode));
        modes.insert("macp.mode.quorum.v1".into(), Box::new(QuorumMode));
        modes.insert("macp.mode.multi_round.v1".into(), Box::new(MultiRoundMode));

        Self {
            registry,
            log_store,
            modes,
        }
    }

    pub fn registered_mode_names(&self) -> Vec<String> {
        standard_mode_names()
            .iter()
            .filter(|mode_name| self.modes.contains_key(**mode_name))
            .map(|mode_name| (*mode_name).to_string())
            .collect()
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
            ModeResponse::PersistState(state) => session.mode_state = state,
            ModeResponse::Resolve(resolution) => {
                session.state = SessionState::Resolved;
                session.resolution = Some(resolution);
            }
            ModeResponse::PersistAndResolve { state, resolution } => {
                session.mode_state = state;
                session.state = SessionState::Resolved;
                session.resolution = Some(resolution);
            }
        }
    }

    async fn persist_sessions(&self, sessions: &HashMap<String, Session>) {
        if let Err(err) = self.registry.persist_locked(sessions).await {
            eprintln!("warning: failed to persist session registry: {err}");
        }
    }

    async fn maybe_expire_session(&self, session_id: &str, session: &mut Session) -> bool {
        let now = Utc::now().timestamp_millis();
        if session.state == SessionState::Open && now > session.ttl_expiry {
            self.log_store
                .append(session_id, Self::make_internal_entry("TtlExpired", b""))
                .await;
            session.state = SessionState::Expired;
            return true;
        }
        false
    }

    pub async fn process(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
        match env.message_type.as_str() {
            "SessionStart" => self.process_session_start(env).await,
            "Signal" => self.process_signal(env).await,
            _ => self.process_message(env).await,
        }
    }

    async fn process_session_start(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
        if env.mode.trim().is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        let mode_name = env.mode.as_str();
        let mode = self.modes.get(mode_name).ok_or(MacpError::UnknownMode)?;

        let start_payload = if env.payload.is_empty() && !is_standard_mode(mode_name) {
            SessionStartPayload::default()
        } else {
            parse_session_start_payload(&env.payload)?
        };
        validate_standard_session_start_payload(mode_name, &start_payload)?;
        let ttl_ms = if is_standard_mode(mode_name) {
            extract_ttl_ms(&start_payload)?
        } else if start_payload.ttl_ms == 0 {
            EXPERIMENTAL_DEFAULT_TTL_MS
        } else {
            extract_ttl_ms(&start_payload)?
        };

        let mut guard = self.registry.sessions.write().await;
        if let Some(existing) = guard.get(&env.session_id) {
            if existing.seen_message_ids.contains(&env.message_id) {
                return Ok(ProcessResult {
                    session_state: existing.state.clone(),
                    duplicate: true,
                });
            }
            return Err(MacpError::DuplicateSession);
        }

        let accepted_at = Utc::now().timestamp_millis();
        let ttl_expiry = accepted_at.saturating_add(ttl_ms);
        let session = Session {
            session_id: env.session_id.clone(),
            state: SessionState::Open,
            ttl_expiry,
            started_at_unix_ms: accepted_at,
            resolution: None,
            mode: mode_name.to_string(),
            mode_state: vec![],
            participants: start_payload.participants.clone(),
            seen_message_ids: std::collections::HashSet::new(),
            intent: start_payload.intent.clone(),
            mode_version: start_payload.mode_version.clone(),
            configuration_version: start_payload.configuration_version.clone(),
            policy_version: start_payload.policy_version.clone(),
            context: start_payload.context.clone(),
            roots: start_payload.roots.clone(),
            initiator_sender: env.sender.clone(),
        };

        let response = mode.on_session_start(&session, env)?;

        self.log_store.create_session_log(&env.session_id).await;
        self.log_store
            .append(&env.session_id, Self::make_incoming_entry(env))
            .await;

        let mut session = session;
        session.seen_message_ids.insert(env.message_id.clone());
        Self::apply_mode_response(&mut session, response);

        // Multi-round mode stores participants in its own state rather than in
        // the session-level field.  This block back-fills session.participants so
        // that authorization checks work uniformly.  It is intentionally coupled
        // to MultiRoundState; if new experimental modes adopt the same pattern
        // this should be generalized.
        if session.participants.is_empty() && !session.mode_state.is_empty() {
            if let Ok(state) = serde_json::from_slice::<crate::mode::multi_round::MultiRoundState>(
                &session.mode_state,
            ) {
                session.participants = state.participants.clone();
            }
        }

        let result_state = session.state.clone();
        guard.insert(env.session_id.clone(), session);
        self.persist_sessions(&guard).await;

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

        if session.seen_message_ids.contains(&env.message_id) {
            return Ok(ProcessResult {
                session_state: session.state.clone(),
                duplicate: true,
            });
        }

        if self.maybe_expire_session(&env.session_id, session).await {
            self.persist_sessions(&guard).await;
            return Err(MacpError::TtlExpired);
        }

        if session.state != SessionState::Open {
            return Err(MacpError::SessionNotOpen);
        }

        let mode = self
            .modes
            .get(&session.mode)
            .ok_or(MacpError::UnknownMode)?;
        mode.authorize_sender(session, env)?;
        let response = mode.on_message(session, env)?;

        session.seen_message_ids.insert(env.message_id.clone());
        self.log_store
            .append(&env.session_id, Self::make_incoming_entry(env))
            .await;
        Self::apply_mode_response(session, response);
        let result_state = session.state.clone();
        self.persist_sessions(&guard).await;

        Ok(ProcessResult {
            session_state: result_state,
            duplicate: false,
        })
    }

    /// Process a Signal envelope.  Signals are defined in the MACP spec for
    /// out-of-band notifications (progress, heartbeat, etc.) but their semantics
    /// are not yet finalized.  This stub accepts any Signal without side-effects
    /// so that compliant clients can send them without error.
    async fn process_signal(&self, _env: &Envelope) -> Result<ProcessResult, MacpError> {
        Ok(ProcessResult {
            session_state: SessionState::Open,
            duplicate: false,
        })
    }

    pub async fn get_session_checked(&self, session_id: &str) -> Option<Session> {
        let mut guard = self.registry.sessions.write().await;
        let changed = if let Some(session) = guard.get_mut(session_id) {
            self.maybe_expire_session(session_id, session).await
        } else {
            return None;
        };
        if changed {
            self.persist_sessions(&guard).await;
        }
        guard.get(session_id).cloned()
    }

    pub async fn cancel_session(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<ProcessResult, MacpError> {
        let mut guard = self.registry.sessions.write().await;
        let session = guard.get_mut(session_id).ok_or(MacpError::UnknownSession)?;

        let _ = self.maybe_expire_session(session_id, session).await;

        if session.state == SessionState::Resolved || session.state == SessionState::Expired {
            let result_state = session.state.clone();
            self.persist_sessions(&guard).await;
            return Ok(ProcessResult {
                session_state: result_state,
                duplicate: false,
            });
        }

        self.log_store
            .append(
                session_id,
                Self::make_internal_entry("SessionCancel", reason.as_bytes()),
            )
            .await;
        session.state = SessionState::Expired;
        self.persist_sessions(&guard).await;

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
    use crate::pb::{CommitmentPayload, SessionStartPayload};
    use prost::Message;

    fn make_runtime() -> Runtime {
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        Runtime::new(registry, log_store)
    }

    fn session_start(participants: Vec<String>) -> Vec<u8> {
        SessionStartPayload {
            intent: "intent".into(),
            participants,
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            ttl_ms: 1_000,
            context: vec![],
            roots: vec![],
        }
        .encode_to_vec()
    }

    fn env(
        mode: &str,
        message_type: &str,
        message_id: &str,
        session_id: &str,
        sender: &str,
        payload: Vec<u8>,
    ) -> Envelope {
        Envelope {
            macp_version: "1.0".into(),
            mode: mode.into(),
            message_type: message_type.into(),
            message_id: message_id.into(),
            session_id: session_id.into(),
            sender: sender.into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload,
        }
    }

    #[tokio::test]
    async fn standard_session_start_is_strict() {
        let rt = make_runtime();
        let bad = SessionStartPayload {
            ttl_ms: 0,
            ..Default::default()
        }
        .encode_to_vec();
        let err = rt
            .process(&env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                "s1",
                "agent://orchestrator",
                bad,
            ))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MacpError::InvalidPayload | MacpError::InvalidTtl
        ));
    }

    #[tokio::test]
    async fn empty_mode_is_rejected() {
        let rt = make_runtime();
        let err = rt
            .process(&env(
                "",
                "SessionStart",
                "m1",
                "s1",
                "agent://orchestrator",
                session_start(vec!["agent://fraud".into()]),
            ))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidEnvelope");
    }

    #[tokio::test]
    async fn rejected_messages_do_not_enter_dedup_state() {
        let rt = make_runtime();
        rt.process(&env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "agent://orchestrator",
            session_start(vec!["agent://fraud".into()]),
        ))
        .await
        .unwrap();

        let bad = rt
            .process(&env(
                "macp.mode.decision.v1",
                "Proposal",
                "m2",
                "s1",
                "agent://fraud",
                b"not-protobuf".to_vec(),
            ))
            .await
            .unwrap_err();
        assert_eq!(bad.to_string(), "InvalidPayload");

        let good = ProposalPayload {
            proposal_id: "p1".into(),
            option: "step-up".into(),
            rationale: "risk".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let result = rt
            .process(&env(
                "macp.mode.decision.v1",
                "Proposal",
                "m2",
                "s1",
                "agent://orchestrator",
                good,
            ))
            .await
            .unwrap();
        assert!(!result.duplicate);
    }

    #[tokio::test]
    async fn get_session_transitions_expired_sessions() {
        let rt = make_runtime();
        let payload = SessionStartPayload {
            intent: "intent".into(),
            participants: vec!["agent://fraud".into()],
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            ttl_ms: 1,
            context: vec![],
            roots: vec![],
        }
        .encode_to_vec();
        rt.process(&env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "agent://orchestrator",
            payload,
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let session = rt.get_session_checked("s1").await.unwrap();
        assert_eq!(session.state, SessionState::Expired);
    }

    #[tokio::test]
    async fn experimental_mode_keeps_legacy_default_ttl() {
        let rt = make_runtime();
        let payload = SessionStartPayload {
            participants: vec!["creator".into(), "other".into()],
            ..Default::default()
        }
        .encode_to_vec();
        rt.process(&env(
            "macp.mode.multi_round.v1",
            "SessionStart",
            "m1",
            "s1",
            "creator",
            payload,
        ))
        .await
        .unwrap();
        let session = rt.get_session_checked("s1").await.unwrap();
        assert!(session.ttl_expiry > session.started_at_unix_ms);
    }

    #[tokio::test]
    async fn duplicate_session_start_message_id_returns_duplicate() {
        let rt = make_runtime();
        let payload = session_start(vec!["agent://fraud".into()]);
        rt.process(&env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "agent://orchestrator",
            payload.clone(),
        ))
        .await
        .unwrap();

        let result = rt
            .process(&env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                "s1",
                "agent://orchestrator",
                payload,
            ))
            .await
            .unwrap();
        assert!(result.duplicate);
    }

    #[tokio::test]
    async fn cancel_idempotent_on_already_expired() {
        let rt = make_runtime();
        let payload = SessionStartPayload {
            intent: "intent".into(),
            participants: vec!["agent://fraud".into()],
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            ttl_ms: 1,
            context: vec![],
            roots: vec![],
        }
        .encode_to_vec();
        rt.process(&env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            "s1",
            "agent://orchestrator",
            payload,
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let result = rt.cancel_session("s1", "cleanup").await.unwrap();
        assert_eq!(result.session_state, SessionState::Expired);
    }

    #[tokio::test]
    async fn commitment_versions_are_carried_into_resolution() {
        let rt = make_runtime();
        rt.process(&env(
            "macp.mode.proposal.v1",
            "SessionStart",
            "m1",
            "s1",
            "agent://buyer",
            session_start(vec!["agent://buyer".into(), "agent://seller".into()]),
        ))
        .await
        .unwrap();

        let proposal = crate::proposal_pb::ProposalPayload {
            proposal_id: "p1".into(),
            title: "offer".into(),
            summary: "summary".into(),
            details: vec![],
            tags: vec![],
        }
        .encode_to_vec();
        rt.process(&env(
            "macp.mode.proposal.v1",
            "Proposal",
            "m2",
            "s1",
            "agent://seller",
            proposal,
        ))
        .await
        .unwrap();
        let accept = crate::proposal_pb::AcceptPayload {
            proposal_id: "p1".into(),
            reason: String::new(),
        }
        .encode_to_vec();
        rt.process(&env(
            "macp.mode.proposal.v1",
            "Accept",
            "m3",
            "s1",
            "agent://seller",
            accept.clone(),
        ))
        .await
        .unwrap();
        rt.process(&env(
            "macp.mode.proposal.v1",
            "Accept",
            "m4",
            "s1",
            "agent://buyer",
            accept,
        ))
        .await
        .unwrap();
        let commitment = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "proposal.accepted".into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy-1".into(),
            configuration_version: "cfg-1".into(),
        }
        .encode_to_vec();
        let result = rt
            .process(&env(
                "macp.mode.proposal.v1",
                "Commitment",
                "m5",
                "s1",
                "agent://buyer",
                commitment,
            ))
            .await
            .unwrap();
        assert_eq!(result.session_state, SessionState::Resolved);
    }
}
