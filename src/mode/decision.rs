use crate::error::MacpError;
use crate::mode::{Mode, ModeResponse};
use crate::pb::Envelope;
use crate::session::Session;

/// DecisionMode wraps the original `payload == b"resolve"` behavior.
/// This preserves backward compatibility for existing clients.
pub struct DecisionMode;

impl Mode for DecisionMode {
    fn on_session_start(
        &self,
        _session: &Session,
        _env: &Envelope,
    ) -> Result<ModeResponse, MacpError> {
        Ok(ModeResponse::NoOp)
    }

    fn on_message(&self, _session: &Session, env: &Envelope) -> Result<ModeResponse, MacpError> {
        if env.payload == b"resolve" {
            Ok(ModeResponse::Resolve(env.payload.clone()))
        } else {
            Ok(ModeResponse::NoOp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;

    fn test_session() -> Session {
        Session {
            session_id: "s1".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            resolution: None,
            mode: "decision".into(),
            mode_state: vec![],
            participants: vec![],
        }
    }

    fn test_envelope(payload: &[u8]) -> Envelope {
        Envelope {
            macp_version: "v1".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn session_start_returns_noop() {
        let mode = DecisionMode;
        let session = test_session();
        let env = Envelope {
            macp_version: "v1".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: vec![],
        };

        let result = mode.on_session_start(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::NoOp));
    }

    #[test]
    fn resolve_payload_returns_resolve() {
        let mode = DecisionMode;
        let session = test_session();
        let env = test_envelope(b"resolve");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::Resolve(_)));
    }

    #[test]
    fn other_payload_returns_noop() {
        let mode = DecisionMode;
        let session = test_session();
        let env = test_envelope(b"hello world");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::NoOp));
    }
}
