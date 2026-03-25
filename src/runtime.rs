use chrono::Utc;
use std::sync::Arc;

use crate::error::MacpError;
use crate::log_store::{EntryKind, LogEntry, LogStore};
use crate::metrics::RuntimeMetrics;
use crate::mode_registry::ModeRegistry;
use crate::pb::{Envelope, ModeDescriptor};
use crate::registry::SessionRegistry;
use crate::session::{
    extract_ttl_ms, parse_session_start_payload, validate_canonical_session_start_payload,
    validate_session_id_for_acceptance, Session, SessionState,
};
use crate::storage::StorageBackend;
use crate::stream_bus::SessionStreamBus;

#[derive(Debug)]
pub struct ProcessResult {
    pub session_state: SessionState,
    pub duplicate: bool,
}

pub struct Runtime {
    pub storage: Arc<dyn StorageBackend>,
    pub registry: Arc<SessionRegistry>,
    pub log_store: Arc<LogStore>,
    stream_bus: Arc<SessionStreamBus>,
    mode_registry: Arc<ModeRegistry>,
    metrics: Arc<RuntimeMetrics>,
}

impl Runtime {
    pub fn new(
        storage: Arc<dyn StorageBackend>,
        registry: Arc<SessionRegistry>,
        log_store: Arc<LogStore>,
    ) -> Self {
        Self::with_mode_registry(
            storage,
            registry,
            log_store,
            Arc::new(ModeRegistry::build_default()),
        )
    }

    pub fn with_mode_registry(
        storage: Arc<dyn StorageBackend>,
        registry: Arc<SessionRegistry>,
        log_store: Arc<LogStore>,
        mode_registry: Arc<ModeRegistry>,
    ) -> Self {
        Self {
            storage,
            registry,
            log_store,
            stream_bus: Arc::new(SessionStreamBus::default()),
            mode_registry,
            metrics: Arc::new(RuntimeMetrics::new()),
        }
    }

    /// Returns all mode names the runtime can handle (standards-track + extensions).
    /// Used by Initialize and GetManifest to advertise full capability.
    pub fn registered_mode_names(&self) -> Vec<String> {
        self.mode_registry.all_mode_names()
    }

    /// Returns only standards-track mode descriptors for ListModes.
    pub fn standard_mode_descriptors(&self) -> Vec<ModeDescriptor> {
        self.mode_registry.standard_mode_descriptors()
    }

    /// Returns only extension mode descriptors for ListExtModes.
    pub fn extension_mode_descriptors(&self) -> Vec<ModeDescriptor> {
        self.mode_registry.extension_mode_descriptors()
    }

    pub fn register_extension(&self, descriptor: ModeDescriptor) -> Result<(), String> {
        self.mode_registry.register_extension(descriptor)
    }

    pub fn unregister_extension(&self, mode: &str) -> Result<(), String> {
        self.mode_registry.unregister_extension(mode)
    }

    pub fn promote_mode(&self, mode: &str, new_name: Option<&str>) -> Result<String, String> {
        self.mode_registry.promote_mode(mode, new_name)
    }

    pub fn subscribe_mode_changes(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.mode_registry.subscribe_changes()
    }

    pub fn mode_registry(&self) -> &Arc<ModeRegistry> {
        &self.mode_registry
    }

    pub fn metrics(&self) -> &Arc<RuntimeMetrics> {
        &self.metrics
    }

    pub fn subscribe_session_stream(
        &self,
        session_id: &str,
    ) -> tokio::sync::broadcast::Receiver<Envelope> {
        self.stream_bus.subscribe(session_id)
    }

    fn publish_accepted_envelope(&self, env: &Envelope) {
        if !env.session_id.is_empty() {
            self.stream_bus.publish(&env.session_id, env.clone());
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
            session_id: env.session_id.clone(),
            mode: env.mode.clone(),
            macp_version: env.macp_version.clone(),
        }
    }

    fn make_internal_entry(
        message_type: &str,
        payload: &[u8],
        session_id: &str,
        mode: &str,
    ) -> LogEntry {
        LogEntry {
            message_id: String::new(),
            received_at_ms: Utc::now().timestamp_millis(),
            sender: "_runtime".into(),
            message_type: message_type.into(),
            raw_payload: payload.to_vec(),
            entry_kind: EntryKind::Internal,
            session_id: session_id.into(),
            mode: mode.into(),
            macp_version: "1.0".into(),
        }
    }

    async fn save_session_to_storage(&self, session: &Session) {
        if let Err(err) = self.storage.save_session(session).await {
            tracing::warn!(
                session_id = %session.session_id,
                error = %err,
                "failed to persist session snapshot"
            );
        }
    }

    async fn maybe_expire_session(
        &self,
        session_id: &str,
        session: &mut Session,
    ) -> Result<bool, MacpError> {
        let now = Utc::now().timestamp_millis();
        if session.state == SessionState::Open && now > session.ttl_expiry {
            let entry = Self::make_internal_entry("TtlExpired", b"", session_id, &session.mode);
            self.storage
                .append_log_entry(session_id, &entry)
                .await
                .map_err(|_| MacpError::StorageFailed)?;
            self.log_store.append(session_id, entry).await;
            session.state = SessionState::Expired;
            self.metrics.record_session_expired(&session.mode);
            tracing::info!(session_id, "session expired via TTL");
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn process(
        &self,
        env: &Envelope,
        max_open_sessions: Option<usize>,
    ) -> Result<ProcessResult, MacpError> {
        match env.message_type.as_str() {
            "SessionStart" => self.process_session_start(env, max_open_sessions).await,
            "Signal" => self.process_signal(env).await,
            _ => self.process_message(env).await,
        }
    }

    async fn process_session_start(
        &self,
        env: &Envelope,
        max_open_sessions: Option<usize>,
    ) -> Result<ProcessResult, MacpError> {
        if env.mode.trim().is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        validate_session_id_for_acceptance(&env.session_id)?;
        let mode_name = env.mode.as_str();
        let mode = self
            .mode_registry
            .get_mode(mode_name)
            .ok_or(MacpError::UnknownMode)?;

        let start_payload = parse_session_start_payload(&env.payload)?;
        let require_complete_start =
            self.mode_registry.is_standard_mode(mode_name) || mode_name == "ext.multi_round.v1";
        if require_complete_start {
            validate_canonical_session_start_payload(&start_payload)?;
        }
        let ttl_ms = extract_ttl_ms(&start_payload)?;

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

        // Enforce max_open_sessions atomically under the write lock to
        // prevent TOCTOU races where concurrent SessionStart requests
        // both pass a read-lock count check before either is inserted.
        if let Some(max_open) = max_open_sessions {
            let now = Utc::now().timestamp_millis();
            let count = guard
                .values()
                .filter(|s| {
                    s.initiator_sender == env.sender
                        && s.state == SessionState::Open
                        && now <= s.ttl_expiry
                })
                .count();
            if count >= max_open {
                return Err(MacpError::RateLimited);
            }
        }

        let accepted_at = Utc::now().timestamp_millis();
        let ttl_expiry = accepted_at.saturating_add(ttl_ms);
        let session = Session {
            session_id: env.session_id.clone(),
            state: SessionState::Open,
            ttl_expiry,
            ttl_ms,
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

        // 1. Create storage directory and write log entry (COMMIT POINT)
        self.storage
            .create_session_storage(&env.session_id)
            .await
            .map_err(|_| MacpError::StorageFailed)?;
        let incoming_entry = Self::make_incoming_entry(env);
        self.storage
            .append_log_entry(&env.session_id, &incoming_entry)
            .await
            .map_err(|_| MacpError::StorageFailed)?;

        // 2. Update in-memory caches
        self.log_store.create_session_log(&env.session_id).await;
        self.log_store.append(&env.session_id, incoming_entry).await;

        let mut session = session;
        session.seen_message_ids.insert(env.message_id.clone());
        session.apply_mode_response(response);

        let result_state = session.state.clone();
        // 3. Best-effort session save
        self.save_session_to_storage(&session).await;
        self.metrics.record_session_start(mode_name);
        tracing::info!(
            session_id = %env.session_id,
            mode = mode_name,
            sender = %env.sender,
            "session started"
        );
        guard.insert(env.session_id.clone(), session);
        self.publish_accepted_envelope(env);

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

        // Validate that the envelope mode matches the session's bound mode.
        // This prevents a token scoped to mode X from sending messages into
        // a session bound to mode Y (server.rs authorizes against env.mode).
        if env.mode != session.mode {
            return Err(MacpError::InvalidEnvelope);
        }

        if self.maybe_expire_session(&env.session_id, session).await? {
            self.save_session_to_storage(session).await;
            return Err(MacpError::TtlExpired);
        }

        if session.state != SessionState::Open {
            return Err(MacpError::SessionNotOpen);
        }

        let mode = self
            .mode_registry
            .get_mode(&session.mode)
            .ok_or(MacpError::UnknownMode)?;
        mode.authorize_sender(session, env)?;
        let response = mode.on_message(session, env)?;

        // 1. COMMIT POINT: write log entry to disk
        let incoming_entry = Self::make_incoming_entry(env);
        self.storage
            .append_log_entry(&env.session_id, &incoming_entry)
            .await
            .map_err(|_| MacpError::StorageFailed)?;

        // 2. Update in-memory state
        self.log_store.append(&env.session_id, incoming_entry).await;
        session.seen_message_ids.insert(env.message_id.clone());
        session.apply_mode_response(response);
        let result_state = session.state.clone();

        self.metrics.record_message_accepted(&session.mode);
        if env.message_type == "Commitment" {
            self.metrics.record_commitment_accepted(&session.mode);
        }

        tracing::debug!(
            session_id = %env.session_id,
            message_type = %env.message_type,
            sender = %env.sender,
            state = ?result_state,
            "message accepted"
        );

        if result_state == SessionState::Resolved {
            self.metrics.record_session_resolved(&session.mode);
            tracing::info!(session_id = %env.session_id, mode = %session.mode, "session resolved");
        }

        // 3. Best-effort session save
        self.save_session_to_storage(session).await;
        self.publish_accepted_envelope(env);

        Ok(ProcessResult {
            session_state: result_state,
            duplicate: false,
        })
    }

    /// Process a Signal envelope. Signals are informational out-of-band
    /// notifications (progress, heartbeat, etc.). Logged but no state mutation.
    async fn process_signal(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
        tracing::debug!(
            sender = %env.sender,
            message_id = %env.message_id,
            "signal received"
        );
        Ok(ProcessResult {
            session_state: SessionState::Open,
            duplicate: false,
        })
    }

    pub async fn get_session_checked(&self, session_id: &str) -> Option<Session> {
        let mut guard = self.registry.sessions.write().await;
        let changed = if let Some(session) = guard.get_mut(session_id) {
            self.maybe_expire_session(session_id, session)
                .await
                .unwrap_or(false)
        } else {
            return None;
        };
        if changed {
            if let Some(session) = guard.get(session_id) {
                self.save_session_to_storage(session).await;
            }
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

        self.maybe_expire_session(session_id, session).await?;

        if session.state == SessionState::Resolved || session.state == SessionState::Expired {
            let result_state = session.state.clone();
            self.save_session_to_storage(session).await;
            return Ok(ProcessResult {
                session_state: result_state,
                duplicate: false,
            });
        }

        let cancel_entry = Self::make_internal_entry(
            "SessionCancel",
            reason.as_bytes(),
            session_id,
            &session.mode,
        );
        self.storage
            .append_log_entry(session_id, &cancel_entry)
            .await
            .map_err(|_| MacpError::StorageFailed)?;
        self.log_store.append(session_id, cancel_entry).await;
        session.state = SessionState::Expired;
        self.save_session_to_storage(session).await;
        self.metrics.record_session_cancelled(&session.mode);
        tracing::info!(session_id, reason, "session cancelled");

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

    fn new_sid() -> String {
        uuid::Uuid::new_v4().as_hyphenated().to_string()
    }

    fn make_runtime() -> Runtime {
        let storage: Arc<dyn StorageBackend> = Arc::new(crate::storage::MemoryBackend);
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        Runtime::new(storage, registry, log_store)
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
        let sid = new_sid();
        let bad = SessionStartPayload {
            ttl_ms: 0,
            ..Default::default()
        }
        .encode_to_vec();
        let err = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "SessionStart",
                    "m1",
                    &sid,
                    "agent://orchestrator",
                    bad,
                ),
                None,
            )
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
        let sid = new_sid();
        let err = rt
            .process(
                &env(
                    "",
                    "SessionStart",
                    "m1",
                    &sid,
                    "agent://orchestrator",
                    session_start(vec!["agent://fraud".into()]),
                ),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidEnvelope");
    }

    #[tokio::test]
    async fn rejected_messages_do_not_enter_dedup_state() {
        let rt = make_runtime();
        let sid = new_sid();
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                session_start(vec!["agent://fraud".into()]),
            ),
            None,
        )
        .await
        .unwrap();

        let bad = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "Proposal",
                    "m2",
                    &sid,
                    "agent://fraud",
                    b"not-protobuf".to_vec(),
                ),
                None,
            )
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
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "Proposal",
                    "m2",
                    &sid,
                    "agent://orchestrator",
                    good,
                ),
                None,
            )
            .await
            .unwrap();
        assert!(!result.duplicate);
    }

    #[tokio::test]
    async fn get_session_transitions_expired_sessions() {
        let rt = make_runtime();
        let sid = new_sid();
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
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                payload,
            ),
            None,
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let session = rt.get_session_checked(&sid).await.unwrap();
        assert_eq!(session.state, SessionState::Expired);
    }

    #[tokio::test]
    async fn multi_round_requires_standard_session_start() {
        let rt = make_runtime();
        let sid = new_sid();
        // multi-round is now standards-track: empty mode_version should fail
        let payload = SessionStartPayload {
            participants: vec!["creator".into(), "other".into()],
            ..Default::default()
        }
        .encode_to_vec();
        let err = rt
            .process(
                &env(
                    "ext.multi_round.v1",
                    "SessionStart",
                    "m1",
                    &sid,
                    "creator",
                    payload,
                ),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            MacpError::InvalidPayload | MacpError::InvalidTtl
        ));
    }

    #[tokio::test]
    async fn multi_round_valid_session_start() {
        let rt = make_runtime();
        let sid = new_sid();
        let payload = session_start(vec!["alice".into(), "bob".into()]);
        rt.process(
            &env(
                "ext.multi_round.v1",
                "SessionStart",
                "m1",
                &sid,
                "coordinator",
                payload,
            ),
            None,
        )
        .await
        .unwrap();
        let session = rt.get_session_checked(&sid).await.unwrap();
        assert_eq!(session.mode, "ext.multi_round.v1");
        assert_eq!(session.participants, vec!["alice", "bob"]);
    }

    #[tokio::test]
    async fn duplicate_session_start_message_id_returns_duplicate() {
        let rt = make_runtime();
        let sid = new_sid();
        let payload = session_start(vec!["agent://fraud".into()]);
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                payload.clone(),
            ),
            None,
        )
        .await
        .unwrap();

        let result = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "SessionStart",
                    "m1",
                    &sid,
                    "agent://orchestrator",
                    payload,
                ),
                None,
            )
            .await
            .unwrap();
        assert!(result.duplicate);
    }

    #[tokio::test]
    async fn non_start_mode_mismatch_rejected() {
        let rt = make_runtime();
        let sid = new_sid();
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                session_start(vec!["agent://fraud".into()]),
            ),
            None,
        )
        .await
        .unwrap();

        let proposal = ProposalPayload {
            proposal_id: "p1".into(),
            option: "step-up".into(),
            rationale: "risk".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let err = rt
            .process(
                &env(
                    "macp.mode.task.v1",
                    "Proposal",
                    "m2",
                    &sid,
                    "agent://orchestrator",
                    proposal,
                ),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidEnvelope");
    }

    #[tokio::test]
    async fn cancel_idempotent_on_already_expired() {
        let rt = make_runtime();
        let sid = new_sid();
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
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                payload,
            ),
            None,
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let result = rt.cancel_session(&sid, "cleanup").await.unwrap();
        assert_eq!(result.session_state, SessionState::Expired);
    }

    #[tokio::test]
    async fn accepted_envelopes_are_published_in_order() {
        let rt = make_runtime();
        let sid = new_sid();
        let mut events = rt.subscribe_session_stream(&sid);

        let start = env(
            "macp.mode.decision.v1",
            "SessionStart",
            "m1",
            &sid,
            "agent://orchestrator",
            session_start(vec!["agent://fraud".into()]),
        );
        rt.process(&start, None).await.unwrap();
        let first = events.recv().await.unwrap();
        assert_eq!(first.message_id, "m1");
        assert_eq!(first.message_type, "SessionStart");

        let proposal = ProposalPayload {
            proposal_id: "p1".into(),
            option: "step-up".into(),
            rationale: "risk".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let proposal_env = env(
            "macp.mode.decision.v1",
            "Proposal",
            "m2",
            &sid,
            "agent://orchestrator",
            proposal,
        );
        rt.process(&proposal_env, None).await.unwrap();
        let second = events.recv().await.unwrap();
        assert_eq!(second.message_id, "m2");
        assert_eq!(second.message_type, "Proposal");
    }

    #[tokio::test]
    async fn commitment_versions_are_carried_into_resolution() {
        let rt = make_runtime();
        let sid = new_sid();
        rt.process(
            &env(
                "macp.mode.proposal.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://buyer",
                session_start(vec!["agent://buyer".into(), "agent://seller".into()]),
            ),
            None,
        )
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
        rt.process(
            &env(
                "macp.mode.proposal.v1",
                "Proposal",
                "m2",
                &sid,
                "agent://seller",
                proposal,
            ),
            None,
        )
        .await
        .unwrap();
        let accept = crate::proposal_pb::AcceptPayload {
            proposal_id: "p1".into(),
            reason: String::new(),
        }
        .encode_to_vec();
        rt.process(
            &env(
                "macp.mode.proposal.v1",
                "Accept",
                "m3",
                &sid,
                "agent://seller",
                accept.clone(),
            ),
            None,
        )
        .await
        .unwrap();
        rt.process(
            &env(
                "macp.mode.proposal.v1",
                "Accept",
                "m4",
                &sid,
                "agent://buyer",
                accept,
            ),
            None,
        )
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
            .process(
                &env(
                    "macp.mode.proposal.v1",
                    "Commitment",
                    "m5",
                    &sid,
                    "agent://buyer",
                    commitment,
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.session_state, SessionState::Resolved);
    }

    #[tokio::test]
    async fn max_open_sessions_enforced_under_write_lock() {
        let rt = make_runtime();
        let sid1 = new_sid();
        let sid2 = new_sid();
        let sid3 = new_sid();
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid1,
                "agent://orchestrator",
                session_start(vec!["agent://fraud".into()]),
            ),
            Some(1),
        )
        .await
        .unwrap();

        let err = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "SessionStart",
                    "m2",
                    &sid2,
                    "agent://orchestrator",
                    session_start(vec!["agent://fraud".into()]),
                ),
                Some(1),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MacpError::RateLimited));

        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m3",
                &sid3,
                "agent://other",
                session_start(vec!["agent://fraud".into()]),
            ),
            Some(1),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn weak_session_id_rejected() {
        let rt = make_runtime();
        let err = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "SessionStart",
                    "m1",
                    "s1",
                    "agent://orchestrator",
                    session_start(vec!["agent://fraud".into()]),
                ),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidSessionId");
    }

    #[tokio::test]
    async fn log_append_failure_rejects_session_start() {
        use std::io;
        struct FailingBackend;
        #[async_trait::async_trait]
        impl StorageBackend for FailingBackend {
            async fn save_session(&self, _: &Session) -> io::Result<()> {
                Ok(())
            }
            async fn load_session(&self, _: &str) -> io::Result<Option<Session>> {
                Ok(None)
            }
            async fn load_all_sessions(&self) -> io::Result<Vec<Session>> {
                Ok(vec![])
            }
            async fn append_log_entry(&self, _: &str, _: &LogEntry) -> io::Result<()> {
                Err(io::Error::other("disk full"))
            }
            async fn load_log(&self, _: &str) -> io::Result<Vec<LogEntry>> {
                Ok(vec![])
            }
            async fn create_session_storage(&self, _: &str) -> io::Result<()> {
                Ok(())
            }
        }

        let storage: Arc<dyn StorageBackend> = Arc::new(FailingBackend);
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let rt = Runtime::new(storage, registry, log_store);
        let sid = new_sid();

        let err = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "SessionStart",
                    "m1",
                    &sid,
                    "agent://orchestrator",
                    session_start(vec!["agent://fraud".into()]),
                ),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "StorageFailed");
    }

    #[tokio::test]
    async fn log_append_failure_rejects_in_session_message() {
        use std::io;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailOnSecondAppend {
            count: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl StorageBackend for FailOnSecondAppend {
            async fn save_session(&self, _: &Session) -> io::Result<()> {
                Ok(())
            }
            async fn load_session(&self, _: &str) -> io::Result<Option<Session>> {
                Ok(None)
            }
            async fn load_all_sessions(&self) -> io::Result<Vec<Session>> {
                Ok(vec![])
            }
            async fn append_log_entry(&self, _: &str, _: &LogEntry) -> io::Result<()> {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                if n >= 1 {
                    Err(io::Error::other("disk full"))
                } else {
                    Ok(())
                }
            }
            async fn load_log(&self, _: &str) -> io::Result<Vec<LogEntry>> {
                Ok(vec![])
            }
            async fn create_session_storage(&self, _: &str) -> io::Result<()> {
                Ok(())
            }
        }

        let storage: Arc<dyn StorageBackend> = Arc::new(FailOnSecondAppend {
            count: AtomicUsize::new(0),
        });
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let rt = Runtime::new(storage, registry, log_store);
        let sid = new_sid();

        // SessionStart succeeds (first append)
        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                session_start(vec!["agent://fraud".into()]),
            ),
            None,
        )
        .await
        .unwrap();

        // Proposal fails (second append)
        let proposal = ProposalPayload {
            proposal_id: "p1".into(),
            option: "step-up".into(),
            rationale: "risk".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let err = rt
            .process(
                &env(
                    "macp.mode.decision.v1",
                    "Proposal",
                    "m2",
                    &sid,
                    "agent://orchestrator",
                    proposal,
                ),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "StorageFailed");

        // Verify the message was not added to dedup state
        let session = rt.get_session_checked(&sid).await.unwrap();
        assert!(!session.seen_message_ids.contains("m2"));
    }

    #[tokio::test]
    async fn cancel_session_fails_if_log_append_fails() {
        use std::io;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailOnSecondAppend {
            count: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl StorageBackend for FailOnSecondAppend {
            async fn save_session(&self, _: &Session) -> io::Result<()> {
                Ok(())
            }
            async fn load_session(&self, _: &str) -> io::Result<Option<Session>> {
                Ok(None)
            }
            async fn load_all_sessions(&self) -> io::Result<Vec<Session>> {
                Ok(vec![])
            }
            async fn append_log_entry(&self, _: &str, _: &LogEntry) -> io::Result<()> {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                if n >= 1 {
                    Err(io::Error::other("disk full"))
                } else {
                    Ok(())
                }
            }
            async fn load_log(&self, _: &str) -> io::Result<Vec<LogEntry>> {
                Ok(vec![])
            }
            async fn create_session_storage(&self, _: &str) -> io::Result<()> {
                Ok(())
            }
        }

        let storage: Arc<dyn StorageBackend> = Arc::new(FailOnSecondAppend {
            count: AtomicUsize::new(0),
        });
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let rt = Runtime::new(storage, registry, log_store);
        let sid = new_sid();

        rt.process(
            &env(
                "macp.mode.decision.v1",
                "SessionStart",
                "m1",
                &sid,
                "agent://orchestrator",
                session_start(vec!["agent://fraud".into()]),
            ),
            None,
        )
        .await
        .unwrap();

        let err = rt.cancel_session(&sid, "test cancel").await.unwrap_err();
        assert_eq!(err.to_string(), "StorageFailed");
    }
}
