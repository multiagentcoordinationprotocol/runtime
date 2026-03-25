use crate::error::MacpError;
use crate::log_store::{EntryKind, LogEntry};
use crate::mode_registry::ModeRegistry;
use crate::pb::Envelope;
use crate::session::{
    extract_ttl_ms, parse_session_start_payload, validate_canonical_session_start_payload, Session,
    SessionState,
};

/// Rebuild a `Session` from its append-only log.
///
/// This replays the same mode callbacks (`on_session_start`, `on_message`) in
/// order so mode state, dedup state, and session lifecycle are reconstructed
/// identically to how they were built during live processing.
pub fn replay_session(
    session_id: &str,
    log_entries: &[LogEntry],
    registry: &ModeRegistry,
) -> Result<Session, MacpError> {
    // 1. Find the SessionStart entry
    let start_entry = log_entries
        .iter()
        .find(|e| e.entry_kind == EntryKind::Incoming && e.message_type == "SessionStart")
        .ok_or(MacpError::InvalidPayload)?;

    // Determine mode: prefer entry-level field, fall back to empty for legacy
    let mode_name = if start_entry.mode.is_empty() {
        // Legacy v2 entry — cannot determine mode from log entry alone;
        // caller should skip or use directory heuristic
        return Err(MacpError::InvalidPayload);
    } else {
        &start_entry.mode
    };

    let mode = registry.get_mode(mode_name).ok_or(MacpError::UnknownMode)?;

    // 2. Parse SessionStartPayload
    let require_complete_start =
        registry.is_standard_mode(mode_name) || mode_name == "ext.multi_round.v1";
    let start_payload = if start_entry.raw_payload.is_empty() && !require_complete_start {
        crate::pb::SessionStartPayload::default()
    } else {
        parse_session_start_payload(&start_entry.raw_payload)?
    };
    if require_complete_start {
        validate_canonical_session_start_payload(&start_payload)?;
    }

    let ttl_ms = if !require_complete_start && start_payload.ttl_ms == 0 {
        // Legacy experimental modes may have 0 ttl_ms
        60_000i64
    } else {
        extract_ttl_ms(&start_payload)?
    };

    // 3. Construct base session — use original received_at_ms, never Utc::now()
    let started_at_unix_ms = start_entry.received_at_ms;
    let ttl_expiry = started_at_unix_ms.saturating_add(ttl_ms);

    let env = Envelope {
        macp_version: if start_entry.macp_version.is_empty() {
            "1.0".into()
        } else {
            start_entry.macp_version.clone()
        },
        mode: mode_name.to_string(),
        message_type: "SessionStart".into(),
        message_id: start_entry.message_id.clone(),
        session_id: session_id.into(),
        sender: start_entry.sender.clone(),
        timestamp_unix_ms: start_entry.received_at_ms,
        payload: start_entry.raw_payload.clone(),
    };

    let mut session = Session {
        session_id: session_id.into(),
        state: SessionState::Open,
        ttl_expiry,
        ttl_ms,
        started_at_unix_ms,
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
        initiator_sender: start_entry.sender.clone(),
    };

    // 4. Call mode.on_session_start(), apply response
    let response = mode.on_session_start(&session, &env)?;
    session.seen_message_ids.insert(env.message_id.clone());
    session.apply_mode_response(response);

    // 5. Replay subsequent entries
    for entry in log_entries.iter().skip(1) {
        match entry.entry_kind {
            EntryKind::Incoming => {
                let replay_env = Envelope {
                    macp_version: if entry.macp_version.is_empty() {
                        "1.0".into()
                    } else {
                        entry.macp_version.clone()
                    },
                    mode: if entry.mode.is_empty() {
                        session.mode.clone()
                    } else {
                        entry.mode.clone()
                    },
                    message_type: entry.message_type.clone(),
                    message_id: entry.message_id.clone(),
                    session_id: session_id.into(),
                    sender: entry.sender.clone(),
                    timestamp_unix_ms: entry.received_at_ms,
                    payload: entry.raw_payload.clone(),
                };

                if session.state != SessionState::Open {
                    // Session already resolved/expired, just rebuild dedup
                    if !replay_env.message_id.is_empty() {
                        session.seen_message_ids.insert(replay_env.message_id);
                    }
                    continue;
                }

                // Replay through the same authorization and mode callbacks used during
                // live processing. Accepted history that no longer replays cleanly must
                // fail recovery instead of silently drifting session state.
                mode.authorize_sender(&session, &replay_env)?;
                let response = mode.on_message(&session, &replay_env)?;
                session.apply_mode_response(response);
                if !replay_env.message_id.is_empty() {
                    session.seen_message_ids.insert(replay_env.message_id);
                }
            }
            EntryKind::Internal => {
                match entry.message_type.as_str() {
                    "TtlExpired" => {
                        session.state = SessionState::Expired;
                    }
                    "SessionCancel" => {
                        session.state = SessionState::Expired;
                    }
                    _ => {
                        // Unknown internal event — skip
                    }
                }
            }
        }
    }

    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision_pb::ProposalPayload;
    use crate::decision_pb::VotePayload;
    use crate::log_store::EntryKind;
    use crate::pb::{CommitmentPayload, SessionStartPayload};
    use prost::Message;

    fn make_registry() -> ModeRegistry {
        ModeRegistry::build_default()
    }

    fn start_payload_bytes() -> Vec<u8> {
        SessionStartPayload {
            intent: "test".into(),
            participants: vec!["agent://fraud".into()],
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            ttl_ms: 60_000,
            context: vec![],
            roots: vec![],
        }
        .encode_to_vec()
    }

    fn incoming_entry(
        message_id: &str,
        message_type: &str,
        sender: &str,
        payload: Vec<u8>,
        received_at_ms: i64,
    ) -> LogEntry {
        LogEntry {
            message_id: message_id.into(),
            received_at_ms,
            sender: sender.into(),
            message_type: message_type.into(),
            raw_payload: payload,
            entry_kind: EntryKind::Incoming,
            session_id: "s1".into(),
            mode: "macp.mode.decision.v1".into(),
            macp_version: "1.0".into(),
        }
    }

    fn internal_entry(message_type: &str, received_at_ms: i64) -> LogEntry {
        LogEntry {
            message_id: String::new(),
            received_at_ms,
            sender: "_runtime".into(),
            message_type: message_type.into(),
            raw_payload: vec![],
            entry_kind: EntryKind::Internal,
            session_id: "s1".into(),
            mode: "macp.mode.decision.v1".into(),
            macp_version: "1.0".into(),
        }
    }

    #[test]
    fn replay_rebuilds_decision_session() {
        let registry = make_registry();
        let proposal = ProposalPayload {
            proposal_id: "p1".into(),
            option: "deploy".into(),
            rationale: "ready".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        let vote = VotePayload {
            proposal_id: "p1".into(),
            vote: "approve".into(),
            reason: "lgtm".into(),
        }
        .encode_to_vec();
        let commitment = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "decision.selected".into(),
            authority_scope: "payments".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy-1".into(),
            configuration_version: "cfg-1".into(),
        }
        .encode_to_vec();

        let entries = vec![
            incoming_entry(
                "m1",
                "SessionStart",
                "agent://orchestrator",
                start_payload_bytes(),
                1000,
            ),
            incoming_entry("m2", "Proposal", "agent://orchestrator", proposal, 2000),
            incoming_entry("m3", "Vote", "agent://fraud", vote, 3000),
            incoming_entry("m4", "Commitment", "agent://orchestrator", commitment, 4000),
        ];

        let session = replay_session("s1", &entries, &registry).unwrap();
        assert_eq!(session.state, SessionState::Resolved);
        assert_eq!(session.session_id, "s1");
        assert!(session.seen_message_ids.contains("m1"));
        assert!(session.seen_message_ids.contains("m2"));
        assert!(session.seen_message_ids.contains("m3"));
        assert!(session.seen_message_ids.contains("m4"));
        assert!(session.resolution.is_some());
    }

    #[test]
    fn replay_preserves_original_ttl() {
        let registry = make_registry();
        let original_time = 1_700_000_000_000i64;
        let entries = vec![incoming_entry(
            "m1",
            "SessionStart",
            "agent://orchestrator",
            start_payload_bytes(),
            original_time,
        )];

        let session = replay_session("s1", &entries, &registry).unwrap();
        assert_eq!(session.started_at_unix_ms, original_time);
        assert_eq!(session.ttl_expiry, original_time + 60_000);
        assert_eq!(session.ttl_ms, 60_000);
    }

    #[test]
    fn replay_handles_ttl_expired() {
        let registry = make_registry();
        let entries = vec![
            incoming_entry(
                "m1",
                "SessionStart",
                "agent://orchestrator",
                start_payload_bytes(),
                1000,
            ),
            internal_entry("TtlExpired", 61001),
        ];

        let session = replay_session("s1", &entries, &registry).unwrap();
        assert_eq!(session.state, SessionState::Expired);
    }

    #[test]
    fn replay_handles_session_cancel() {
        let registry = make_registry();
        let entries = vec![
            incoming_entry(
                "m1",
                "SessionStart",
                "agent://orchestrator",
                start_payload_bytes(),
                1000,
            ),
            internal_entry("SessionCancel", 5000),
        ];

        let session = replay_session("s1", &entries, &registry).unwrap();
        assert_eq!(session.state, SessionState::Expired);
    }

    #[test]
    fn replay_fails_when_accepted_history_no_longer_applies() {
        let registry = make_registry();
        let vote = VotePayload {
            proposal_id: "p1".into(),
            vote: "approve".into(),
            reason: String::new(),
        }
        .encode_to_vec();
        let entries = vec![
            incoming_entry(
                "m1",
                "SessionStart",
                "agent://orchestrator",
                start_payload_bytes(),
                1000,
            ),
            incoming_entry("m2", "Vote", "agent://fraud", vote, 2000),
        ];

        let err = replay_session("s1", &entries, &registry).unwrap_err();
        // The exact error variant depends on which check fails first (authorize_sender
        // or on_message); what matters is that replay does NOT silently succeed.
        let msg = err.to_string();
        assert!(
            msg == "InvalidTransition" || msg == "InvalidPayload" || msg == "Forbidden",
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn replay_empty_log_returns_error() {
        let registry = make_registry();
        let result = replay_session("s1", &[], &registry);
        assert!(result.is_err());
    }

    #[test]
    fn backward_compat_old_log_entry_without_new_fields() {
        // Simulate deserializing a v2 log entry without session_id/mode/macp_version
        let json = r#"{"message_id":"m1","received_at_ms":1000,"sender":"test","message_type":"Message","raw_payload":[],"entry_kind":"Incoming"}"#;
        let entry: LogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.session_id, "");
        assert_eq!(entry.mode, "");
        assert_eq!(entry.macp_version, "");
    }
}
