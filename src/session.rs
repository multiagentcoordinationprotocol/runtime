use crate::error::MacpError;
use crate::pb::SessionStartPayload;
use prost::Message;
use std::collections::HashSet;

pub const DEFAULT_TTL_MS: i64 = 60_000;
pub const MAX_TTL_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq)]
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
    // RFC version fields from SessionStartPayload
    pub intent: String,
    pub mode_version: String,
    pub configuration_version: String,
    pub policy_version: String,
}

/// Parse a protobuf-encoded SessionStartPayload from raw bytes.
pub fn parse_session_start_payload(payload: &[u8]) -> Result<SessionStartPayload, MacpError> {
    if payload.is_empty() {
        return Ok(SessionStartPayload::default());
    }
    SessionStartPayload::decode(payload).map_err(|_| MacpError::InvalidPayload)
}

/// Extract and validate TTL from a parsed SessionStartPayload.
pub fn extract_ttl_ms(payload: &SessionStartPayload) -> Result<i64, MacpError> {
    if payload.ttl_ms == 0 {
        return Ok(DEFAULT_TTL_MS);
    }
    if !(1..=MAX_TTL_MS).contains(&payload.ttl_ms) {
        return Err(MacpError::InvalidTtl);
    }
    Ok(payload.ttl_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn encode_payload(ttl_ms: i64, participants: Vec<String>) -> Vec<u8> {
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

    #[test]
    fn parse_empty_payload_returns_default() {
        let result = parse_session_start_payload(b"").unwrap();
        assert_eq!(result.ttl_ms, 0);
        assert!(result.participants.is_empty());
    }

    #[test]
    fn parse_valid_protobuf_payload() {
        let bytes = encode_payload(5000, vec!["alice".into(), "bob".into()]);
        let result = parse_session_start_payload(&bytes).unwrap();
        assert_eq!(result.ttl_ms, 5000);
        assert_eq!(result.participants, vec!["alice", "bob"]);
    }

    #[test]
    fn parse_invalid_bytes_returns_error() {
        let err = parse_session_start_payload(b"not protobuf").unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn extract_ttl_default_when_zero() {
        let payload = SessionStartPayload::default();
        assert_eq!(extract_ttl_ms(&payload).unwrap(), DEFAULT_TTL_MS);
    }

    #[test]
    fn extract_ttl_valid() {
        let payload = SessionStartPayload {
            ttl_ms: 5000,
            ..Default::default()
        };
        assert_eq!(extract_ttl_ms(&payload).unwrap(), 5000);
    }

    #[test]
    fn extract_ttl_boundary_min() {
        let payload = SessionStartPayload {
            ttl_ms: 1,
            ..Default::default()
        };
        assert_eq!(extract_ttl_ms(&payload).unwrap(), 1);
    }

    #[test]
    fn extract_ttl_boundary_max() {
        let payload = SessionStartPayload {
            ttl_ms: MAX_TTL_MS,
            ..Default::default()
        };
        assert_eq!(extract_ttl_ms(&payload).unwrap(), MAX_TTL_MS);
    }

    #[test]
    fn extract_ttl_negative_returns_invalid() {
        let payload = SessionStartPayload {
            ttl_ms: -5000,
            ..Default::default()
        };
        let err = extract_ttl_ms(&payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    #[test]
    fn extract_ttl_exceeds_max_returns_invalid() {
        let payload = SessionStartPayload {
            ttl_ms: MAX_TTL_MS + 1,
            ..Default::default()
        };
        let err = extract_ttl_ms(&payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    #[test]
    fn parse_payload_with_context_bytes() {
        let payload = SessionStartPayload {
            intent: "test intent".into(),
            ttl_ms: 10_000,
            participants: vec!["alice".into()],
            mode_version: "1.0".into(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: b"some context data".to_vec(),
            roots: vec![],
        };
        let bytes = payload.encode_to_vec();
        let result = parse_session_start_payload(&bytes).unwrap();
        assert_eq!(result.ttl_ms, 10_000);
        assert_eq!(result.participants, vec!["alice"]);
        assert_eq!(result.intent, "test intent");
        assert_eq!(result.mode_version, "1.0");
        assert_eq!(result.context, b"some context data");
    }

    #[test]
    fn parse_payload_with_only_participants() {
        let payload = SessionStartPayload {
            ttl_ms: 0,
            participants: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        };
        let bytes = payload.encode_to_vec();
        let result = parse_session_start_payload(&bytes).unwrap();
        assert_eq!(result.ttl_ms, 0);
        assert_eq!(result.participants.len(), 3);
    }

    #[test]
    fn extract_ttl_at_minus_one_returns_invalid() {
        let payload = SessionStartPayload {
            ttl_ms: -1,
            ..Default::default()
        };
        let err = extract_ttl_ms(&payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    #[test]
    fn session_state_equality() {
        assert_eq!(SessionState::Open, SessionState::Open);
        assert_eq!(SessionState::Resolved, SessionState::Resolved);
        assert_eq!(SessionState::Expired, SessionState::Expired);
        assert_ne!(SessionState::Open, SessionState::Resolved);
        assert_ne!(SessionState::Open, SessionState::Expired);
        assert_ne!(SessionState::Resolved, SessionState::Expired);
    }
}
