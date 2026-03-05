use crate::error::MacpError;
use serde::Deserialize;

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
    pub resolution: Option<Vec<u8>>,
    pub mode: String,
    pub mode_state: Vec<u8>,
    pub participants: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SessionStartConfig {
    ttl_ms: Option<i64>,
}

pub fn parse_session_start_ttl_ms(payload: &[u8]) -> Result<i64, MacpError> {
    if payload.is_empty() {
        return Ok(DEFAULT_TTL_MS);
    }

    let text = std::str::from_utf8(payload).map_err(|_| MacpError::InvalidEnvelope)?;
    let config: SessionStartConfig =
        serde_json::from_str(text).map_err(|_| MacpError::InvalidEnvelope)?;

    match config.ttl_ms {
        None => Ok(DEFAULT_TTL_MS),
        Some(ms) if !(1..=MAX_TTL_MS).contains(&ms) => Err(MacpError::InvalidTtl),
        Some(ms) => Ok(ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ttl_empty_payload_returns_default() {
        let result = parse_session_start_ttl_ms(b"");
        assert_eq!(result.unwrap(), DEFAULT_TTL_MS);
    }

    #[test]
    fn parse_ttl_valid_json_with_ttl() {
        let payload = br#"{"ttl_ms": 5000}"#;
        assert_eq!(parse_session_start_ttl_ms(payload).unwrap(), 5000);
    }

    #[test]
    fn parse_ttl_json_missing_field_returns_default() {
        let payload = br#"{}"#;
        assert_eq!(parse_session_start_ttl_ms(payload).unwrap(), DEFAULT_TTL_MS);
    }

    #[test]
    fn parse_ttl_json_null_field_returns_default() {
        let payload = br#"{"ttl_ms": null}"#;
        assert_eq!(parse_session_start_ttl_ms(payload).unwrap(), DEFAULT_TTL_MS);
    }

    #[test]
    fn parse_ttl_boundary_min_valid() {
        let payload = br#"{"ttl_ms": 1}"#;
        assert_eq!(parse_session_start_ttl_ms(payload).unwrap(), 1);
    }

    #[test]
    fn parse_ttl_boundary_max_valid() {
        let payload = format!(r#"{{"ttl_ms": {}}}"#, MAX_TTL_MS);
        assert_eq!(
            parse_session_start_ttl_ms(payload.as_bytes()).unwrap(),
            MAX_TTL_MS
        );
    }

    #[test]
    fn parse_ttl_zero_returns_invalid() {
        let payload = br#"{"ttl_ms": 0}"#;
        let err = parse_session_start_ttl_ms(payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    #[test]
    fn parse_ttl_negative_returns_invalid() {
        let payload = br#"{"ttl_ms": -5000}"#;
        let err = parse_session_start_ttl_ms(payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    #[test]
    fn parse_ttl_exceeds_max_returns_invalid() {
        let payload = format!(r#"{{"ttl_ms": {}}}"#, MAX_TTL_MS + 1);
        let err = parse_session_start_ttl_ms(payload.as_bytes()).unwrap_err();
        assert_eq!(err.to_string(), "InvalidTtl");
    }

    #[test]
    fn parse_ttl_invalid_utf8_returns_invalid_envelope() {
        let payload: &[u8] = &[0xff, 0xfe, 0xfd];
        let err = parse_session_start_ttl_ms(payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidEnvelope");
    }

    #[test]
    fn parse_ttl_invalid_json_returns_invalid_envelope() {
        let payload = b"not json at all";
        let err = parse_session_start_ttl_ms(payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidEnvelope");
    }

    #[test]
    fn parse_ttl_wrong_type_returns_invalid_envelope() {
        let payload = br#"{"ttl_ms": "five thousand"}"#;
        let err = parse_session_start_ttl_ms(payload).unwrap_err();
        assert_eq!(err.to_string(), "InvalidEnvelope");
    }
}
