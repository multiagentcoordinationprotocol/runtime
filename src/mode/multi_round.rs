use crate::error::MacpError;
use crate::mode::{Mode, ModeResponse};
use crate::pb::Envelope;
use crate::session::Session;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Internal state tracked across rounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiRoundState {
    pub round: u64,
    pub participants: Vec<String>,
    pub contributions: BTreeMap<String, String>,
    #[serde(default)]
    pub convergence_type: String,
}

/// Payload for Contribute messages.
#[derive(Debug, Clone, Deserialize)]
struct ContributePayload {
    value: String,
}

/// Resolution payload emitted on convergence.
#[derive(Debug, Serialize)]
struct ResolutionPayload {
    converged_value: String,
    round: u64,
    #[serde(rename = "final")]
    final_values: BTreeMap<String, String>,
}

pub struct MultiRoundMode;

impl MultiRoundMode {
    fn encode_state(state: &MultiRoundState) -> Vec<u8> {
        serde_json::to_vec(state).expect("MultiRoundState is always serializable")
    }

    fn decode_state(data: &[u8]) -> Result<MultiRoundState, MacpError> {
        serde_json::from_slice(data).map_err(|_| MacpError::InvalidModeState)
    }
}

impl Mode for MultiRoundMode {
    fn on_session_start(
        &self,
        session: &Session,
        _env: &Envelope,
    ) -> Result<ModeResponse, MacpError> {
        // Participants come from the runtime via SessionStartPayload
        let participants = session.participants.clone();

        if participants.is_empty() {
            return Err(MacpError::InvalidPayload);
        }

        let state = MultiRoundState {
            round: 0,
            participants,
            contributions: BTreeMap::new(),
            convergence_type: "all_equal".into(),
        };

        Ok(ModeResponse::PersistState(Self::encode_state(&state)))
    }

    fn on_message(&self, session: &Session, env: &Envelope) -> Result<ModeResponse, MacpError> {
        if env.message_type != "Contribute" {
            return Ok(ModeResponse::NoOp);
        }

        let mut state = Self::decode_state(&session.mode_state)?;

        let text = std::str::from_utf8(&env.payload).map_err(|_| MacpError::InvalidPayload)?;
        let contribute: ContributePayload =
            serde_json::from_str(text).map_err(|_| MacpError::InvalidPayload)?;

        // Check if the value changed from previous contribution
        let previous = state.contributions.get(&env.sender);
        let value_changed = previous.is_none_or(|prev| *prev != contribute.value);

        if value_changed {
            state.round += 1;
            state
                .contributions
                .insert(env.sender.clone(), contribute.value);
        }

        // Check convergence: all participants contributed + all values identical
        let all_contributed = state
            .participants
            .iter()
            .all(|p| state.contributions.contains_key(p));

        if all_contributed {
            let values: Vec<&String> = state.contributions.values().collect();
            let all_equal = values.windows(2).all(|w| w[0] == w[1]);

            if all_equal {
                let converged_value = values[0].clone();
                let resolution = ResolutionPayload {
                    converged_value,
                    round: state.round,
                    final_values: state.contributions.clone(),
                };
                let resolution_bytes = serde_json::to_vec(&resolution)
                    .expect("ResolutionPayload is always serializable");
                return Ok(ModeResponse::PersistAndResolve {
                    state: Self::encode_state(&state),
                    resolution: resolution_bytes,
                });
            }
        }

        Ok(ModeResponse::PersistState(Self::encode_state(&state)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use std::collections::HashSet;

    fn base_session() -> Session {
        Session {
            session_id: "s1".into(),
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
        }
    }

    fn session_start_env() -> Envelope {
        Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "s1".into(),
            sender: "creator".into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: vec![],
        }
    }

    fn contribute_env(sender: &str, value: &str) -> Envelope {
        let payload = serde_json::json!({"value": value}).to_string();
        Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: format!("m_{}", sender),
            session_id: "s1".into(),
            sender: sender.into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: payload.into_bytes(),
        }
    }

    fn session_with_state(state: &MultiRoundState) -> Session {
        let mut s = base_session();
        s.mode_state = MultiRoundMode::encode_state(state);
        s.participants = state.participants.clone();
        s
    }

    #[test]
    fn session_start_parses_valid_config() {
        let mode = MultiRoundMode;
        let mut session = base_session();
        session.participants = vec!["alice".into(), "bob".into()];
        let env = session_start_env();

        let result = mode.on_session_start(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: MultiRoundState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.round, 0);
                assert_eq!(state.participants, vec!["alice", "bob"]);
                assert!(state.contributions.is_empty());
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn session_start_rejects_empty_participants() {
        let mode = MultiRoundMode;
        let session = base_session(); // empty participants
        let env = session_start_env();

        let err = mode.on_session_start(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn contribute_first_value_increments_round() {
        let mode = MultiRoundMode;
        let state = MultiRoundState {
            round: 0,
            participants: vec!["alice".into(), "bob".into()],
            contributions: BTreeMap::new(),
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("alice", "option_a");

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let new_state: MultiRoundState = serde_json::from_slice(&data).unwrap();
                assert_eq!(new_state.round, 1);
                assert_eq!(new_state.contributions.get("alice").unwrap(), "option_a");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn resubmit_same_value_does_not_increment_round() {
        let mode = MultiRoundMode;
        let mut contributions = BTreeMap::new();
        contributions.insert("alice".to_string(), "option_a".to_string());
        let state = MultiRoundState {
            round: 1,
            participants: vec!["alice".into(), "bob".into()],
            contributions,
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("alice", "option_a");

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let new_state: MultiRoundState = serde_json::from_slice(&data).unwrap();
                assert_eq!(new_state.round, 1); // unchanged
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn revise_value_increments_round() {
        let mode = MultiRoundMode;
        let mut contributions = BTreeMap::new();
        contributions.insert("alice".to_string(), "option_a".to_string());
        let state = MultiRoundState {
            round: 1,
            participants: vec!["alice".into(), "bob".into()],
            contributions,
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("alice", "option_b");

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let new_state: MultiRoundState = serde_json::from_slice(&data).unwrap();
                assert_eq!(new_state.round, 2);
                assert_eq!(new_state.contributions.get("alice").unwrap(), "option_b");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn convergence_when_all_equal() {
        let mode = MultiRoundMode;
        let mut contributions = BTreeMap::new();
        contributions.insert("alice".to_string(), "option_a".to_string());
        let state = MultiRoundState {
            round: 1,
            participants: vec!["alice".into(), "bob".into()],
            contributions,
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("bob", "option_a");

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistAndResolve {
                state: state_bytes,
                resolution,
            } => {
                let final_state: MultiRoundState = serde_json::from_slice(&state_bytes).unwrap();
                assert_eq!(final_state.round, 2);

                let res: serde_json::Value = serde_json::from_slice(&resolution).unwrap();
                assert_eq!(res["converged_value"], "option_a");
                assert_eq!(res["round"], 2);
                assert_eq!(res["final"]["alice"], "option_a");
                assert_eq!(res["final"]["bob"], "option_a");
            }
            _ => panic!("Expected PersistAndResolve"),
        }
    }

    #[test]
    fn no_convergence_when_values_differ() {
        let mode = MultiRoundMode;
        let mut contributions = BTreeMap::new();
        contributions.insert("alice".to_string(), "option_a".to_string());
        let state = MultiRoundState {
            round: 1,
            participants: vec!["alice".into(), "bob".into()],
            contributions,
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("bob", "option_b");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::PersistState(_)));
    }

    #[test]
    fn no_convergence_when_not_all_contributed() {
        let mode = MultiRoundMode;
        let state = MultiRoundState {
            round: 0,
            participants: vec!["alice".into(), "bob".into(), "carol".into()],
            contributions: BTreeMap::new(),
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("alice", "option_a");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::PersistState(_)));
    }

    #[test]
    fn non_contribute_message_returns_noop() {
        let mode = MultiRoundMode;
        let state = MultiRoundState {
            round: 0,
            participants: vec!["alice".into()],
            contributions: BTreeMap::new(),
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "alice".into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: b"hello".to_vec(),
        };

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::NoOp));
    }

    #[test]
    fn contribute_invalid_payload_returns_error() {
        let mode = MultiRoundMode;
        let state = MultiRoundState {
            round: 0,
            participants: vec!["alice".into()],
            contributions: BTreeMap::new(),
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "alice".into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: b"not json".to_vec(),
        };

        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn encode_decode_round_trip() {
        let mut contributions = BTreeMap::new();
        contributions.insert("alice".into(), "value_a".into());
        let original = MultiRoundState {
            round: 5,
            participants: vec!["alice".into(), "bob".into()],
            contributions,
            convergence_type: "all_equal".into(),
        };

        let encoded = MultiRoundMode::encode_state(&original);
        let decoded = MultiRoundMode::decode_state(&encoded).unwrap();

        assert_eq!(decoded.round, original.round);
        assert_eq!(decoded.participants, original.participants);
        assert_eq!(decoded.contributions, original.contributions);
    }

    #[test]
    fn decode_invalid_state_returns_error() {
        let err = MultiRoundMode::decode_state(b"garbage").unwrap_err();
        assert_eq!(err.to_string(), "InvalidModeState");
    }

    #[test]
    fn three_participant_convergence() {
        let mode = MultiRoundMode;

        let mut contributions = BTreeMap::new();
        contributions.insert("alice".to_string(), "option_a".to_string());
        contributions.insert("bob".to_string(), "option_a".to_string());
        let state = MultiRoundState {
            round: 2,
            participants: vec!["alice".into(), "bob".into(), "carol".into()],
            contributions,
            convergence_type: "all_equal".into(),
        };
        let session = session_with_state(&state);
        let env = contribute_env("carol", "option_a");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::PersistAndResolve { .. }));
    }
}
