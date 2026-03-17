use crate::error::MacpError;
use crate::pb::SessionStartPayload;
use prost::Message;
use std::collections::HashSet;

pub const MAX_TTL_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SessionState {
    Open,
    Resolved,
    Expired,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub session_id: String,
    pub state: SessionState,
    pub ttl_expiry: i64,
    pub started_at_unix_ms: i64,
    pub resolution: Option<Vec<u8>>,
    pub mode: String,
    pub mode_state: Vec<u8>,
    pub participants: Vec<String>,
    pub seen_message_ids: HashSet<String>,
    pub intent: String,
    pub mode_version: String,
    pub configuration_version: String,
    pub policy_version: String,
    pub context: Vec<u8>,
    pub roots: Vec<crate::pb::Root>,
    pub initiator_sender: String,
}

pub fn is_standard_mode(mode: &str) -> bool {
    matches!(
        mode,
        "macp.mode.decision.v1"
            | "macp.mode.proposal.v1"
            | "macp.mode.task.v1"
            | "macp.mode.handoff.v1"
            | "macp.mode.quorum.v1"
    )
}

/// Parse a protobuf-encoded SessionStartPayload from raw bytes.
pub fn parse_session_start_payload(payload: &[u8]) -> Result<SessionStartPayload, MacpError> {
    if payload.is_empty() {
        return Err(MacpError::InvalidPayload);
    }
    SessionStartPayload::decode(payload).map_err(|_| MacpError::InvalidPayload)
}

/// Extract and validate TTL from a parsed SessionStartPayload.
pub fn extract_ttl_ms(payload: &SessionStartPayload) -> Result<i64, MacpError> {
    if !(1..=MAX_TTL_MS).contains(&payload.ttl_ms) {
        return Err(MacpError::InvalidTtl);
    }
    Ok(payload.ttl_ms)
}

/// Enforce the canonical SessionStart binding contract for standards-track modes.
pub fn validate_standard_session_start_payload(
    mode: &str,
    payload: &SessionStartPayload,
) -> Result<(), MacpError> {
    if !is_standard_mode(mode) {
        return Ok(());
    }

    extract_ttl_ms(payload)?;

    if payload.mode_version.trim().is_empty() || payload.configuration_version.trim().is_empty() {
        return Err(MacpError::InvalidPayload);
    }

    if payload.participants.is_empty() {
        return Err(MacpError::InvalidPayload);
    }

    let mut seen = HashSet::new();
    for participant in &payload.participants {
        let participant = participant.trim();
        if participant.is_empty() || !seen.insert(participant.to_string()) {
            return Err(MacpError::InvalidPayload);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn encode_payload(ttl_ms: i64, participants: Vec<String>) -> Vec<u8> {
        let payload = SessionStartPayload {
            intent: String::new(),
            participants,
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: String::new(),
            ttl_ms,
            context: vec![],
            roots: vec![],
        };
        payload.encode_to_vec()
    }

    #[test]
    fn parse_empty_payload_is_invalid() {
        let err = parse_session_start_payload(b"").unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn parse_valid_protobuf_payload() {
        let bytes = encode_payload(5000, vec!["alice".into(), "bob".into()]);
        let result = parse_session_start_payload(&bytes).unwrap();
        assert_eq!(result.ttl_ms, 5000);
        assert_eq!(result.participants, vec!["alice", "bob"]);
    }

    #[test]
    fn extract_ttl_requires_explicit_positive_value() {
        let payload = SessionStartPayload::default();
        assert_eq!(
            extract_ttl_ms(&payload).unwrap_err().to_string(),
            "InvalidTtl"
        );

        let payload = SessionStartPayload {
            ttl_ms: 5000,
            ..Default::default()
        };
        assert_eq!(extract_ttl_ms(&payload).unwrap(), 5000);
    }

    #[test]
    fn standard_mode_requires_explicit_versions_and_participants() {
        let payload = SessionStartPayload {
            participants: vec!["alice".into()],
            mode_version: String::new(),
            configuration_version: "cfg-1".into(),
            ttl_ms: 1000,
            ..Default::default()
        };
        assert_eq!(
            validate_standard_session_start_payload("macp.mode.decision.v1", &payload)
                .unwrap_err()
                .to_string(),
            "InvalidPayload"
        );

        let payload = SessionStartPayload {
            participants: vec![],
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            ttl_ms: 1000,
            ..Default::default()
        };
        assert_eq!(
            validate_standard_session_start_payload("macp.mode.decision.v1", &payload)
                .unwrap_err()
                .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn standard_mode_rejects_duplicate_participants() {
        let payload = SessionStartPayload {
            participants: vec!["alice".into(), "alice".into()],
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            ttl_ms: 1000,
            ..Default::default()
        };
        assert_eq!(
            validate_standard_session_start_payload("macp.mode.proposal.v1", &payload)
                .unwrap_err()
                .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn experimental_modes_keep_legacy_flexibility() {
        let payload = SessionStartPayload::default();
        validate_standard_session_start_payload("macp.mode.multi_round.v1", &payload).unwrap();
    }
}
