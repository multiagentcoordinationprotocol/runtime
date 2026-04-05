use crate::decision_pb::{EvaluationPayload, ObjectionPayload, ProposalPayload, VotePayload};
use crate::error::MacpError;
use crate::mode::util::{is_declared_participant, validate_commitment_payload_for_session};
use crate::mode::{Mode, ModeResponse};
use crate::pb::Envelope;
use crate::session::Session;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum DecisionPhase {
    #[default]
    Proposal,
    Evaluation,
    Voting,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionState {
    pub proposals: BTreeMap<String, Proposal>,
    pub evaluations: Vec<Evaluation>,
    pub objections: Vec<Objection>,
    pub votes: BTreeMap<String, BTreeMap<String, Vote>>,
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

pub struct DecisionMode;

impl DecisionMode {
    fn default_state() -> DecisionState {
        DecisionState {
            proposals: BTreeMap::new(),
            evaluations: Vec::new(),
            objections: Vec::new(),
            votes: BTreeMap::new(),
            phase: DecisionPhase::Proposal,
        }
    }

    fn encode_state(state: &DecisionState) -> Vec<u8> {
        serde_json::to_vec(state).expect("DecisionState is always serializable")
    }

    fn decode_state(data: &[u8]) -> Result<DecisionState, MacpError> {
        serde_json::from_slice(data).map_err(|_| MacpError::InvalidModeState)
    }

    fn ensure_not_committed(state: &DecisionState) -> Result<(), MacpError> {
        if state.phase == DecisionPhase::Committed {
            Err(MacpError::SessionNotOpen)
        } else {
            Ok(())
        }
    }

    fn ensure_known_proposal(state: &DecisionState, proposal_id: &str) -> Result<(), MacpError> {
        if !state.proposals.contains_key(proposal_id) {
            return Err(MacpError::InvalidPayload);
        }
        Ok(())
    }

    fn ensure_can_propose(state: &DecisionState) -> Result<(), MacpError> {
        Self::ensure_not_committed(state)?;
        if state.phase == DecisionPhase::Voting {
            return Err(MacpError::InvalidPayload);
        }
        Ok(())
    }

    fn ensure_can_deliberate(state: &DecisionState) -> Result<(), MacpError> {
        Self::ensure_not_committed(state)?;
        if state.phase != DecisionPhase::Evaluation {
            return Err(MacpError::InvalidPayload);
        }
        Ok(())
    }

    fn ensure_can_vote(state: &DecisionState) -> Result<(), MacpError> {
        Self::ensure_not_committed(state)?;
        if matches!(
            state.phase,
            DecisionPhase::Proposal | DecisionPhase::Committed
        ) {
            return Err(MacpError::InvalidPayload);
        }
        Ok(())
    }

    fn commitment_ready(state: &DecisionState) -> bool {
        !state.proposals.is_empty()
    }
}

impl Mode for DecisionMode {
    /// Authorize the sender for decision mode messages.
    ///
    /// Implements the coordinator authority model reflected in the Decision RFC:
    /// the session initiator may emit `Proposal` and `Commitment` regardless of
    /// declared participants. Declared participants may emit `Proposal`,
    /// `Evaluation`, `Objection`, and `Vote`. Only the initiator may emit
    /// `Commitment`.
    fn authorize_sender(&self, session: &Session, env: &Envelope) -> Result<(), MacpError> {
        match env.message_type.as_str() {
            "Proposal" | "Commitment" if env.sender == session.initiator_sender => Ok(()),
            "Proposal" | "Evaluation" | "Objection" | "Vote"
                if is_declared_participant(&session.participants, &env.sender) =>
            {
                Ok(())
            }
            "Commitment" => Err(MacpError::Forbidden),
            _ => Err(MacpError::Forbidden),
        }
    }

    fn on_session_start(
        &self,
        session: &Session,
        _env: &Envelope,
    ) -> Result<ModeResponse, MacpError> {
        if session.participants.is_empty() {
            return Err(MacpError::InvalidPayload);
        }
        Ok(ModeResponse::PersistState(Self::encode_state(
            &Self::default_state(),
        )))
    }

    fn on_message(&self, session: &Session, env: &Envelope) -> Result<ModeResponse, MacpError> {
        let mut state = if session.mode_state.is_empty() {
            Self::default_state()
        } else {
            Self::decode_state(&session.mode_state)?
        };

        Self::ensure_not_committed(&state)?;

        match env.message_type.as_str() {
            "Proposal" => {
                Self::ensure_can_propose(&state)?;
                let payload = ProposalPayload::decode(&*env.payload)
                    .map_err(|_| MacpError::InvalidPayload)?;
                if payload.proposal_id.trim().is_empty()
                    || payload.option.trim().is_empty()
                    || state.proposals.contains_key(&payload.proposal_id)
                {
                    return Err(MacpError::InvalidPayload);
                }
                state.proposals.insert(
                    payload.proposal_id.clone(),
                    Proposal {
                        proposal_id: payload.proposal_id,
                        option: payload.option,
                        rationale: payload.rationale,
                        sender: env.sender.clone(),
                    },
                );
                state.phase = DecisionPhase::Evaluation;
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Evaluation" => {
                let payload = EvaluationPayload::decode(&*env.payload)
                    .map_err(|_| MacpError::InvalidPayload)?;
                Self::ensure_can_deliberate(&state)?;
                Self::ensure_known_proposal(&state, &payload.proposal_id)?;
                state.evaluations.push(Evaluation {
                    proposal_id: payload.proposal_id,
                    recommendation: payload.recommendation,
                    confidence: payload.confidence,
                    reason: payload.reason,
                    sender: env.sender.clone(),
                });
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Objection" => {
                let payload = ObjectionPayload::decode(&*env.payload)
                    .map_err(|_| MacpError::InvalidPayload)?;
                Self::ensure_can_deliberate(&state)?;
                Self::ensure_known_proposal(&state, &payload.proposal_id)?;
                state.objections.push(Objection {
                    proposal_id: payload.proposal_id,
                    reason: payload.reason,
                    severity: if payload.severity.is_empty() {
                        "medium".into()
                    } else {
                        payload.severity
                    },
                    sender: env.sender.clone(),
                });
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Vote" => {
                let payload =
                    VotePayload::decode(&*env.payload).map_err(|_| MacpError::InvalidPayload)?;
                Self::ensure_can_vote(&state)?;
                Self::ensure_known_proposal(&state, &payload.proposal_id)?;
                let proposal_votes = state.votes.entry(payload.proposal_id.clone()).or_default();
                if proposal_votes.contains_key(&env.sender) {
                    return Err(MacpError::InvalidPayload);
                }
                proposal_votes.insert(
                    env.sender.clone(),
                    Vote {
                        proposal_id: payload.proposal_id,
                        vote: payload.vote,
                        reason: payload.reason,
                        sender: env.sender.clone(),
                    },
                );
                state.phase = DecisionPhase::Voting;
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Commitment" => {
                validate_commitment_payload_for_session(session, &env.payload)?;
                if !Self::commitment_ready(&state) {
                    return Err(MacpError::InvalidPayload);
                }
                state.phase = DecisionPhase::Committed;
                Ok(ModeResponse::PersistAndResolve {
                    state: Self::encode_state(&state),
                    resolution: env.payload.clone(),
                })
            }
            _ => Err(MacpError::InvalidPayload),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pb::CommitmentPayload;
    use crate::session::SessionState;
    use std::collections::HashSet;

    fn test_session() -> Session {
        Session {
            session_id: "s1".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            ttl_ms: 60_000,
            started_at_unix_ms: 0,
            resolution: None,
            mode: "macp.mode.decision.v1".into(),
            mode_state: vec![],
            participants: vec!["agent://fraud".into(), "agent://growth".into()],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            context: vec![],
            roots: vec![],
            initiator_sender: "agent://orchestrator".into(),
            participant_message_counts: std::collections::HashMap::new(),
            participant_last_seen: std::collections::HashMap::new(),
        }
    }

    fn env(sender: &str, message_type: &str, payload: Vec<u8>) -> Envelope {
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.decision.v1".into(),
            message_type: message_type.into(),
            message_id: format!("{}-{}", sender, message_type),
            session_id: "s1".into(),
            sender: sender.into(),
            timestamp_unix_ms: 0,
            payload,
        }
    }

    fn proposal(id: &str) -> Vec<u8> {
        ProposalPayload {
            proposal_id: id.into(),
            option: format!("option-{id}"),
            rationale: "because".into(),
            supporting_data: vec![],
        }
        .encode_to_vec()
    }

    fn vote(id: &str, value: &str) -> Vec<u8> {
        VotePayload {
            proposal_id: id.into(),
            vote: value.into(),
            reason: String::new(),
        }
        .encode_to_vec()
    }

    fn evaluation(proposal_id: &str) -> Vec<u8> {
        EvaluationPayload {
            proposal_id: proposal_id.into(),
            recommendation: "proceed".into(),
            confidence: 0.9,
            reason: "good".into(),
        }
        .encode_to_vec()
    }

    fn objection(proposal_id: &str) -> Vec<u8> {
        ObjectionPayload {
            proposal_id: proposal_id.into(),
            reason: "risky".into(),
            severity: "high".into(),
        }
        .encode_to_vec()
    }

    fn commitment(session: &Session) -> Vec<u8> {
        CommitmentPayload {
            commitment_id: "c1".into(),
            action: "decision.selected".into(),
            authority_scope: "payments".into(),
            reason: "bound".into(),
            mode_version: session.mode_version.clone(),
            policy_version: session.policy_version.clone(),
            configuration_version: session.configuration_version.clone(),
        }
        .encode_to_vec()
    }

    fn apply(session: &mut Session, response: ModeResponse) {
        match response {
            ModeResponse::PersistState(data) => session.mode_state = data,
            ModeResponse::PersistAndResolve { state, .. } => session.mode_state = state,
            _ => {}
        }
    }

    fn decode(session: &Session) -> DecisionState {
        serde_json::from_slice(&session.mode_state).unwrap()
    }

    #[test]
    fn session_start_requires_declared_participants() {
        let mode = DecisionMode;
        let mut session = test_session();
        session.participants.clear();
        assert_eq!(
            mode.on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![])
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn initiator_can_propose_without_being_declared_participant() {
        let mode = DecisionMode;
        let session = test_session();
        mode.authorize_sender(
            &session,
            &env("agent://orchestrator", "Proposal", proposal("p1")),
        )
        .unwrap();
    }

    #[test]
    fn duplicate_proposal_id_is_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(&session, &env("agent://fraud", "Proposal", proposal("p1")))
                .unwrap_err()
                .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn vote_is_scoped_per_proposal() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(&session, &env("agent://fraud", "Proposal", proposal("p2")))
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "approve")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p2", "approve")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "reject"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn evaluation_before_any_proposal_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://fraud", "Evaluation", evaluation("p1"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn objection_before_any_proposal_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://fraud", "Objection", objection("p1"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn vote_before_any_proposal_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "approve"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn proposal_after_voting_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "approve")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p2"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn evaluation_after_voting_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "approve")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://growth", "Evaluation", evaluation("p1"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn objection_after_voting_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "approve")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://growth", "Objection", objection("p1"))
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn commitment_from_non_initiator_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.authorize_sender(
                &session,
                &env("agent://fraud", "Commitment", commitment(&session))
            )
            .unwrap_err()
            .to_string(),
            "Forbidden"
        );
    }

    #[test]
    fn empty_proposal_id_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let empty_proposal = ProposalPayload {
            proposal_id: "".into(),
            option: "option".into(),
            rationale: "because".into(),
            supporting_data: vec![],
        }
        .encode_to_vec();
        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://orchestrator", "Proposal", empty_proposal)
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn malformed_vote_payload_rejected() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(
            mode.on_message(&session, &env("agent://fraud", "Vote", vec![0xff, 0x00]))
                .unwrap_err()
                .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn phase_advances_from_proposal_to_voting_to_committed() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(decode(&session).phase, DecisionPhase::Proposal);

        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(decode(&session).phase, DecisionPhase::Evaluation);

        let resp = mode
            .on_message(
                &session,
                &env("agent://fraud", "Vote", vote("p1", "approve")),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(decode(&session).phase, DecisionPhase::Voting);

        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Commitment", commitment(&session)),
            )
            .unwrap();
        apply(&mut session, resp);
        assert_eq!(decode(&session).phase, DecisionPhase::Committed);
    }

    #[test]
    fn commitment_versions_must_match_session_bindings() {
        let mode = DecisionMode;
        let mut session = test_session();
        let resp = mode
            .on_session_start(
                &session,
                &env("agent://orchestrator", "SessionStart", vec![]),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://orchestrator", "Proposal", proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);

        let mut bad = CommitmentPayload {
            commitment_id: "c1".into(),
            action: "decision.selected".into(),
            authority_scope: "payments".into(),
            reason: "bound".into(),
            mode_version: "wrong".into(),
            policy_version: session.policy_version.clone(),
            configuration_version: session.configuration_version.clone(),
        }
        .encode_to_vec();

        assert_eq!(
            mode.on_message(
                &session,
                &env("agent://orchestrator", "Commitment", bad.clone())
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );

        bad = commitment(&session);
        mode.on_message(&session, &env("agent://orchestrator", "Commitment", bad))
            .unwrap();
    }
}
