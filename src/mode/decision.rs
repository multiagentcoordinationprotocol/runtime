use crate::error::MacpError;
use crate::mode::{Mode, ModeResponse};
use crate::pb::Envelope;
use crate::session::Session;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Phase of the decision lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DecisionPhase {
    Proposal,
    Evaluation,
    Voting,
    Committed,
}

/// Internal state tracked across the decision lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionState {
    pub proposals: HashMap<String, Proposal>,
    pub evaluations: Vec<Evaluation>,
    pub objections: Vec<Objection>,
    pub votes: HashMap<String, Vote>,
    pub phase: DecisionPhase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub proposal_id: String,
    pub option: String,
    pub rationale: String,
    pub sender: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub proposal_id: String,
    pub recommendation: String,
    pub confidence: f64,
    pub reason: String,
    pub sender: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Objection {
    pub proposal_id: String,
    pub reason: String,
    pub severity: String,
    pub sender: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub proposal_id: String,
    pub vote: String,
    pub reason: String,
    pub sender: String,
}

/// DecisionMode implements the RFC-compliant Proposal -> Evaluation -> Vote -> Commitment lifecycle.
/// Also supports the legacy `payload == b"resolve"` behavior for backward compatibility.
pub struct DecisionMode;

impl DecisionMode {
    fn encode_state(state: &DecisionState) -> Vec<u8> {
        serde_json::to_vec(state).expect("DecisionState is always serializable")
    }

    fn decode_state(data: &[u8]) -> Result<DecisionState, MacpError> {
        serde_json::from_slice(data).map_err(|_| MacpError::InvalidModeState)
    }
}

impl Mode for DecisionMode {
    fn on_session_start(
        &self,
        _session: &Session,
        _env: &Envelope,
    ) -> Result<ModeResponse, MacpError> {
        let state = DecisionState {
            proposals: HashMap::new(),
            evaluations: Vec::new(),
            objections: Vec::new(),
            votes: HashMap::new(),
            phase: DecisionPhase::Proposal,
        };
        Ok(ModeResponse::PersistState(Self::encode_state(&state)))
    }

    fn on_message(&self, session: &Session, env: &Envelope) -> Result<ModeResponse, MacpError> {
        // Legacy backward compatibility: payload == "resolve" resolves immediately
        if env.message_type == "Message" && env.payload == b"resolve" {
            return Ok(ModeResponse::Resolve(env.payload.clone()));
        }

        // For non-typed messages, just pass through
        match env.message_type.as_str() {
            "Proposal" | "Evaluation" | "Objection" | "Vote" | "Commitment" => {}
            _ => return Ok(ModeResponse::NoOp),
        }

        let mut state = if session.mode_state.is_empty() {
            DecisionState {
                proposals: HashMap::new(),
                evaluations: Vec::new(),
                objections: Vec::new(),
                votes: HashMap::new(),
                phase: DecisionPhase::Proposal,
            }
        } else {
            Self::decode_state(&session.mode_state)?
        };

        if state.phase == DecisionPhase::Committed {
            return Err(MacpError::SessionNotOpen);
        }

        match env.message_type.as_str() {
            "Proposal" => {
                let payload: ProposalInput =
                    serde_json::from_slice(&env.payload).map_err(|_| MacpError::InvalidPayload)?;
                if payload.proposal_id.is_empty() {
                    return Err(MacpError::InvalidPayload);
                }
                state.proposals.insert(
                    payload.proposal_id.clone(),
                    Proposal {
                        proposal_id: payload.proposal_id,
                        option: payload.option,
                        rationale: payload.rationale.unwrap_or_default(),
                        sender: env.sender.clone(),
                    },
                );
                state.phase = DecisionPhase::Evaluation;
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Evaluation" => {
                let payload: EvaluationInput =
                    serde_json::from_slice(&env.payload).map_err(|_| MacpError::InvalidPayload)?;
                if !state.proposals.contains_key(&payload.proposal_id) {
                    return Err(MacpError::InvalidPayload);
                }
                state.evaluations.push(Evaluation {
                    proposal_id: payload.proposal_id,
                    recommendation: payload.recommendation,
                    confidence: payload.confidence.unwrap_or(0.0),
                    reason: payload.reason.unwrap_or_default(),
                    sender: env.sender.clone(),
                });
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Objection" => {
                let payload: ObjectionInput =
                    serde_json::from_slice(&env.payload).map_err(|_| MacpError::InvalidPayload)?;
                if !state.proposals.contains_key(&payload.proposal_id) {
                    return Err(MacpError::InvalidPayload);
                }
                state.objections.push(Objection {
                    proposal_id: payload.proposal_id,
                    reason: payload.reason,
                    severity: payload.severity.unwrap_or_else(|| "medium".into()),
                    sender: env.sender.clone(),
                });
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Vote" => {
                if state.proposals.is_empty() {
                    return Err(MacpError::InvalidPayload);
                }
                let payload: VoteInput =
                    serde_json::from_slice(&env.payload).map_err(|_| MacpError::InvalidPayload)?;
                if !state.proposals.contains_key(&payload.proposal_id) {
                    return Err(MacpError::InvalidPayload);
                }
                state.votes.insert(
                    env.sender.clone(),
                    Vote {
                        proposal_id: payload.proposal_id,
                        vote: payload.vote,
                        reason: payload.reason.unwrap_or_default(),
                        sender: env.sender.clone(),
                    },
                );
                state.phase = DecisionPhase::Voting;
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Commitment" => {
                if state.votes.is_empty() {
                    return Err(MacpError::InvalidPayload);
                }
                state.phase = DecisionPhase::Committed;
                Ok(ModeResponse::PersistAndResolve {
                    state: Self::encode_state(&state),
                    resolution: env.payload.clone(),
                })
            }
            _ => Ok(ModeResponse::NoOp),
        }
    }
}

// Input types for JSON deserialization from payload
#[derive(Deserialize)]
struct ProposalInput {
    proposal_id: String,
    option: String,
    rationale: Option<String>,
}

#[derive(Deserialize)]
struct EvaluationInput {
    proposal_id: String,
    recommendation: String,
    confidence: Option<f64>,
    reason: Option<String>,
}

#[derive(Deserialize)]
struct ObjectionInput {
    proposal_id: String,
    reason: String,
    severity: Option<String>,
}

#[derive(Deserialize)]
struct VoteInput {
    proposal_id: String,
    vote: String,
    reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use std::collections::HashSet;

    fn test_session() -> Session {
        Session {
            session_id: "s1".into(),
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
        }
    }

    fn test_envelope(message_type: &str, payload: &[u8]) -> Envelope {
        Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: message_type.into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: 1_700_000_000_000,
            payload: payload.to_vec(),
        }
    }

    fn session_with_state(state: &DecisionState) -> Session {
        let mut s = test_session();
        s.mode_state = DecisionMode::encode_state(state);
        s
    }

    fn empty_state() -> DecisionState {
        DecisionState {
            proposals: HashMap::new(),
            evaluations: Vec::new(),
            objections: Vec::new(),
            votes: HashMap::new(),
            phase: DecisionPhase::Proposal,
        }
    }

    fn state_with_proposal() -> DecisionState {
        let mut state = empty_state();
        state.proposals.insert(
            "p1".into(),
            Proposal {
                proposal_id: "p1".into(),
                option: "option_a".into(),
                rationale: "because".into(),
                sender: "alice".into(),
            },
        );
        state.phase = DecisionPhase::Evaluation;
        state
    }

    fn state_with_vote() -> DecisionState {
        let mut state = state_with_proposal();
        state.votes.insert(
            "alice".into(),
            Vote {
                proposal_id: "p1".into(),
                vote: "approve".into(),
                reason: String::new(),
                sender: "alice".into(),
            },
        );
        state.phase = DecisionPhase::Voting;
        state
    }

    #[test]
    fn session_start_initializes_state() {
        let mode = DecisionMode;
        let session = test_session();
        let env = test_envelope("SessionStart", b"");

        let result = mode.on_session_start(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: DecisionState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.phase, DecisionPhase::Proposal);
                assert!(state.proposals.is_empty());
            }
            _ => panic!("Expected PersistState"),
        }
    }

    // --- Legacy backward compatibility ---

    #[test]
    fn legacy_resolve_payload_still_works() {
        let mode = DecisionMode;
        let session = test_session();
        let env = test_envelope("Message", b"resolve");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::Resolve(_)));
    }

    #[test]
    fn other_message_payload_returns_noop() {
        let mode = DecisionMode;
        let session = test_session();
        let env = test_envelope("Message", b"hello world");

        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::NoOp));
    }

    // --- Proposal ---

    #[test]
    fn proposal_creates_entry_and_advances_phase() {
        let mode = DecisionMode;
        let session = session_with_state(&empty_state());
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "option": "option_a",
            "rationale": "it's the best"
        });
        let env = test_envelope("Proposal", payload.to_string().as_bytes());

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: DecisionState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.phase, DecisionPhase::Evaluation);
                assert!(state.proposals.contains_key("p1"));
                let p = &state.proposals["p1"];
                assert_eq!(p.option, "option_a");
                assert_eq!(p.sender, "test");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn proposal_with_empty_id_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&empty_state());
        let payload = serde_json::json!({
            "proposal_id": "",
            "option": "opt"
        });
        let env = test_envelope("Proposal", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn proposal_with_bad_json_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&empty_state());
        let env = test_envelope("Proposal", b"not json");
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Evaluation ---

    #[test]
    fn evaluation_for_existing_proposal() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "recommendation": "APPROVE",
            "confidence": 0.9,
            "reason": "looks good"
        });
        let env = test_envelope("Evaluation", payload.to_string().as_bytes());

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: DecisionState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.evaluations.len(), 1);
                assert_eq!(state.evaluations[0].recommendation, "APPROVE");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn evaluation_for_nonexistent_proposal_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "nonexistent",
            "recommendation": "APPROVE"
        });
        let env = test_envelope("Evaluation", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Objection ---

    #[test]
    fn objection_for_existing_proposal() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "reason": "too risky",
            "severity": "high"
        });
        let env = test_envelope("Objection", payload.to_string().as_bytes());

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: DecisionState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.objections.len(), 1);
                assert_eq!(state.objections[0].severity, "high");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn objection_for_nonexistent_proposal_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "nope",
            "reason": "bad"
        });
        let env = test_envelope("Objection", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Vote ---

    #[test]
    fn vote_for_existing_proposal() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "vote": "approve",
            "reason": "I agree"
        });
        let env = test_envelope("Vote", payload.to_string().as_bytes());

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: DecisionState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.phase, DecisionPhase::Voting);
                assert!(state.votes.contains_key("test"));
                assert_eq!(state.votes["test"].vote, "approve");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn vote_before_proposal_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&empty_state());
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "vote": "approve"
        });
        let env = test_envelope("Vote", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn vote_for_nonexistent_proposal_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "nope",
            "vote": "approve"
        });
        let env = test_envelope("Vote", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn vote_overwrites_previous_vote_by_sender() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "vote": "approve"
        });
        let env = test_envelope("Vote", payload.to_string().as_bytes());
        let result = mode.on_message(&session, &env).unwrap();
        let data = match result {
            ModeResponse::PersistState(d) => d,
            _ => panic!("Expected PersistState"),
        };

        // Second vote by same sender
        let mut session2 = test_session();
        session2.mode_state = data;
        let payload2 = serde_json::json!({
            "proposal_id": "p1",
            "vote": "reject"
        });
        let env2 = test_envelope("Vote", payload2.to_string().as_bytes());
        let result2 = mode.on_message(&session2, &env2).unwrap();
        match result2 {
            ModeResponse::PersistState(data) => {
                let state: DecisionState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.votes.len(), 1);
                assert_eq!(state.votes["test"].vote, "reject");
            }
            _ => panic!("Expected PersistState"),
        }
    }

    // --- Commitment ---

    #[test]
    fn commitment_resolves_session() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_vote());
        let payload = serde_json::json!({
            "commitment_id": "c1",
            "action": "deploy option_a",
            "authority_scope": "team-alpha"
        });
        let env = test_envelope("Commitment", payload.to_string().as_bytes());

        let result = mode.on_message(&session, &env).unwrap();
        match result {
            ModeResponse::PersistAndResolve { state, resolution } => {
                let final_state: DecisionState = serde_json::from_slice(&state).unwrap();
                assert_eq!(final_state.phase, DecisionPhase::Committed);
                assert!(!resolution.is_empty());
            }
            _ => panic!("Expected PersistAndResolve"),
        }
    }

    #[test]
    fn commitment_without_votes_rejected() {
        let mode = DecisionMode;
        let session = session_with_state(&state_with_proposal());
        let payload = serde_json::json!({
            "commitment_id": "c1",
            "action": "deploy"
        });
        let env = test_envelope("Commitment", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Full lifecycle ---

    #[test]
    fn full_decision_lifecycle() {
        let mode = DecisionMode;
        let mut session = test_session();

        // SessionStart
        let env = test_envelope("SessionStart", b"");
        let result = mode.on_session_start(&session, &env).unwrap();
        if let ModeResponse::PersistState(data) = result {
            session.mode_state = data;
        }

        // Proposal
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "option": "option_a",
            "rationale": "best choice"
        });
        let env = test_envelope("Proposal", payload.to_string().as_bytes());
        let result = mode.on_message(&session, &env).unwrap();
        if let ModeResponse::PersistState(data) = result {
            session.mode_state = data;
        }

        // Evaluation
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "recommendation": "APPROVE",
            "confidence": 0.95
        });
        let env = test_envelope("Evaluation", payload.to_string().as_bytes());
        let result = mode.on_message(&session, &env).unwrap();
        if let ModeResponse::PersistState(data) = result {
            session.mode_state = data;
        }

        // Vote
        let payload = serde_json::json!({
            "proposal_id": "p1",
            "vote": "approve",
            "reason": "agreed"
        });
        let env = test_envelope("Vote", payload.to_string().as_bytes());
        let result = mode.on_message(&session, &env).unwrap();
        if let ModeResponse::PersistState(data) = result {
            session.mode_state = data;
        }

        // Commitment
        let payload = serde_json::json!({
            "commitment_id": "c1",
            "action": "deploy option_a",
            "authority_scope": "team",
            "reason": "consensus reached"
        });
        let env = test_envelope("Commitment", payload.to_string().as_bytes());
        let result = mode.on_message(&session, &env).unwrap();
        assert!(matches!(result, ModeResponse::PersistAndResolve { .. }));
    }

    #[test]
    fn message_after_commitment_rejected() {
        let mode = DecisionMode;
        let mut state = state_with_vote();
        state.phase = DecisionPhase::Committed;
        let session = session_with_state(&state);

        let payload = serde_json::json!({
            "proposal_id": "p1",
            "option": "option_b"
        });
        let env = test_envelope("Proposal", payload.to_string().as_bytes());
        let err = mode.on_message(&session, &env).unwrap_err();
        assert_eq!(err.to_string(), "SessionNotOpen");
    }
}
