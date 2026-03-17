use crate::error::MacpError;
use crate::mode::util::{decode_commitment_payload, is_declared_participant};
use crate::mode::{Mode, ModeResponse};
use crate::pb::Envelope;
use crate::proposal_pb::{
    AcceptPayload, CounterProposalPayload, ProposalPayload, RejectPayload, WithdrawPayload,
};
use crate::session::Session;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProposalDisposition {
    Live,
    Withdrawn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRecord {
    pub proposal_id: String,
    pub title: String,
    pub summary: String,
    pub details: Vec<u8>,
    pub tags: Vec<String>,
    pub proposer: String,
    pub supersedes_proposal_id: Option<String>,
    pub disposition: ProposalDisposition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalRejectRecord {
    pub proposal_id: String,
    pub sender: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProposalState {
    pub proposals: BTreeMap<String, ProposalRecord>,
    pub accepts: BTreeMap<String, String>,
    pub terminal_rejections: Vec<TerminalRejectRecord>,
}

pub struct ProposalMode;

impl ProposalMode {
    fn encode_state(state: &ProposalState) -> Vec<u8> {
        serde_json::to_vec(state).expect("ProposalState is always serializable")
    }

    fn decode_state(data: &[u8]) -> Result<ProposalState, MacpError> {
        serde_json::from_slice(data).map_err(|_| MacpError::InvalidModeState)
    }

    fn live_proposal<'a>(
        state: &'a ProposalState,
        proposal_id: &str,
    ) -> Option<&'a ProposalRecord> {
        state
            .proposals
            .get(proposal_id)
            .filter(|record| record.disposition == ProposalDisposition::Live)
    }
}

impl Mode for ProposalMode {
    fn authorize_sender(&self, session: &Session, env: &Envelope) -> Result<(), MacpError> {
        match env.message_type.as_str() {
            "Commitment" if env.sender == session.initiator_sender => Ok(()),
            "Commitment" => Err(MacpError::Forbidden),
            _ if is_declared_participant(&session.participants, &env.sender) => Ok(()),
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
            &ProposalState::default(),
        )))
    }

    fn on_message(&self, session: &Session, env: &Envelope) -> Result<ModeResponse, MacpError> {
        let mut state = if session.mode_state.is_empty() {
            ProposalState::default()
        } else {
            Self::decode_state(&session.mode_state)?
        };

        match env.message_type.as_str() {
            "Proposal" => {
                let payload = ProposalPayload::decode(&*env.payload)
                    .map_err(|_| MacpError::InvalidPayload)?;
                if payload.proposal_id.is_empty()
                    || state.proposals.contains_key(&payload.proposal_id)
                {
                    return Err(MacpError::InvalidPayload);
                }
                state.proposals.insert(
                    payload.proposal_id.clone(),
                    ProposalRecord {
                        proposal_id: payload.proposal_id,
                        title: payload.title,
                        summary: payload.summary,
                        details: payload.details,
                        tags: payload.tags,
                        proposer: env.sender.clone(),
                        supersedes_proposal_id: None,
                        disposition: ProposalDisposition::Live,
                    },
                );
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "CounterProposal" => {
                let payload = CounterProposalPayload::decode(&*env.payload)
                    .map_err(|_| MacpError::InvalidPayload)?;
                if payload.proposal_id.is_empty()
                    || payload.supersedes_proposal_id.is_empty()
                    || state.proposals.contains_key(&payload.proposal_id)
                    || !state
                        .proposals
                        .contains_key(&payload.supersedes_proposal_id)
                {
                    return Err(MacpError::InvalidPayload);
                }
                state.proposals.insert(
                    payload.proposal_id.clone(),
                    ProposalRecord {
                        proposal_id: payload.proposal_id,
                        title: payload.title,
                        summary: payload.summary,
                        details: payload.details,
                        tags: Vec::new(),
                        proposer: env.sender.clone(),
                        supersedes_proposal_id: Some(payload.supersedes_proposal_id),
                        disposition: ProposalDisposition::Live,
                    },
                );
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Accept" => {
                let payload =
                    AcceptPayload::decode(&*env.payload).map_err(|_| MacpError::InvalidPayload)?;
                if Self::live_proposal(&state, &payload.proposal_id).is_none() {
                    return Err(MacpError::InvalidPayload);
                }
                state
                    .accepts
                    .insert(env.sender.clone(), payload.proposal_id);
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Reject" => {
                let payload =
                    RejectPayload::decode(&*env.payload).map_err(|_| MacpError::InvalidPayload)?;
                if !state.proposals.contains_key(&payload.proposal_id) {
                    return Err(MacpError::InvalidPayload);
                }
                if payload.terminal {
                    state.terminal_rejections.push(TerminalRejectRecord {
                        proposal_id: payload.proposal_id,
                        sender: env.sender.clone(),
                        reason: payload.reason,
                    });
                    return Ok(ModeResponse::PersistState(Self::encode_state(&state)));
                }
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Withdraw" => {
                let payload = WithdrawPayload::decode(&*env.payload)
                    .map_err(|_| MacpError::InvalidPayload)?;
                let record = state
                    .proposals
                    .get_mut(&payload.proposal_id)
                    .ok_or(MacpError::InvalidPayload)?;
                if record.proposer != env.sender {
                    return Err(MacpError::Forbidden);
                }
                if record.disposition == ProposalDisposition::Withdrawn {
                    return Err(MacpError::InvalidPayload);
                }
                record.disposition = ProposalDisposition::Withdrawn;
                // Clear acceptances for withdrawn proposal
                state.accepts.retain(|_, pid| pid != &payload.proposal_id);
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Commitment" => {
                if env.sender != session.initiator_sender {
                    return Err(MacpError::Forbidden);
                }
                let _payload = decode_commitment_payload(&env.payload)?;
                if state.proposals.is_empty() {
                    return Err(MacpError::InvalidPayload);
                }
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
    use crate::session::{Session, SessionState};
    use std::collections::HashSet;

    fn base_session() -> Session {
        Session {
            session_id: "s1".into(),
            state: SessionState::Open,
            ttl_expiry: i64::MAX,
            started_at_unix_ms: 0,
            resolution: None,
            mode: "macp.mode.proposal.v1".into(),
            mode_state: vec![],
            participants: vec!["buyer".into(), "seller".into()],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: vec![],
            roots: vec![],
            initiator_sender: "buyer".into(),
        }
    }

    fn env(sender: &str, message_type: &str, payload: Vec<u8>) -> Envelope {
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.proposal.v1".into(),
            message_type: message_type.into(),
            message_id: format!("{}-{}", sender, message_type),
            session_id: "s1".into(),
            sender: sender.into(),
            timestamp_unix_ms: 0,
            payload,
        }
    }

    fn commitment_payload() -> Vec<u8> {
        CommitmentPayload {
            commitment_id: "c1".into(),
            action: "proposal.accepted".into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: "1.0.0".into(),
            policy_version: "policy".into(),
            configuration_version: "config".into(),
        }
        .encode_to_vec()
    }

    fn apply(session: &mut Session, result: ModeResponse) {
        match result {
            ModeResponse::PersistState(data) => session.mode_state = data,
            ModeResponse::PersistAndResolve { state, .. } => session.mode_state = state,
            _ => {}
        }
    }

    fn make_proposal(id: &str, title: &str, summary: &str) -> Vec<u8> {
        ProposalPayload {
            proposal_id: id.into(),
            title: title.into(),
            summary: summary.into(),
            details: vec![],
            tags: vec![],
        }
        .encode_to_vec()
    }

    fn make_counter(id: &str, supersedes: &str, title: &str, summary: &str) -> Vec<u8> {
        CounterProposalPayload {
            proposal_id: id.into(),
            supersedes_proposal_id: supersedes.into(),
            title: title.into(),
            summary: summary.into(),
            details: vec![],
        }
        .encode_to_vec()
    }

    fn make_accept(proposal_id: &str, reason: &str) -> Vec<u8> {
        AcceptPayload {
            proposal_id: proposal_id.into(),
            reason: reason.into(),
        }
        .encode_to_vec()
    }

    fn make_reject(proposal_id: &str, terminal: bool, reason: &str) -> Vec<u8> {
        RejectPayload {
            proposal_id: proposal_id.into(),
            terminal,
            reason: reason.into(),
        }
        .encode_to_vec()
    }

    fn make_withdraw(proposal_id: &str, reason: &str) -> Vec<u8> {
        WithdrawPayload {
            proposal_id: proposal_id.into(),
            reason: reason.into(),
        }
        .encode_to_vec()
    }

    // --- Session Start ---

    #[test]
    fn session_start_initializes_state() {
        let mode = ProposalMode;
        let session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: ProposalState = serde_json::from_slice(&data).unwrap();
                assert!(state.proposals.is_empty());
                assert!(state.accepts.is_empty());
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn session_start_requires_participants() {
        let mode = ProposalMode;
        let mut session = base_session();
        session.participants.clear();
        let err = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Proposal ---

    #[test]
    fn proposal_creates_entry() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);

        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: ProposalState = serde_json::from_slice(&data).unwrap();
                assert!(state.proposals.contains_key("p1"));
                assert_eq!(state.proposals["p1"].proposer, "seller");
                assert_eq!(state.proposals["p1"].disposition, ProposalDisposition::Live);
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn proposal_with_empty_id_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("", "Offer", "1200")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn duplicate_proposal_id_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env("buyer", "Proposal", make_proposal("p1", "Same", "1300")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- CounterProposal ---

    #[test]
    fn counter_proposal_works() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env(
                    "buyer",
                    "CounterProposal",
                    make_counter("p2", "p1", "Counter", "1000"),
                ),
            )
            .unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: ProposalState = serde_json::from_slice(&data).unwrap();
                assert!(state.proposals.contains_key("p2"));
                assert_eq!(
                    state.proposals["p2"].supersedes_proposal_id,
                    Some("p1".into())
                );
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn counter_proposal_missing_supersedes_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env(
                    "buyer",
                    "CounterProposal",
                    make_counter("p2", "nonexistent", "Counter", "1000"),
                ),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Accept ---

    #[test]
    fn accept_live_proposal() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("buyer", "Accept", make_accept("p1", "agree")),
            )
            .unwrap();
        match result {
            ModeResponse::PersistState(data) => {
                let state: ProposalState = serde_json::from_slice(&data).unwrap();
                assert_eq!(state.accepts.get("buyer"), Some(&"p1".to_string()));
            }
            _ => panic!("Expected PersistState"),
        }
    }

    #[test]
    fn accept_supersedes_previous() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer A", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("buyer", "Proposal", make_proposal("p2", "Offer B", "1000")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(&session, &env("buyer", "Accept", make_accept("p1", "ok")))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("buyer", "Accept", make_accept("p2", "changed mind")),
            )
            .unwrap();
        apply(&mut session, result);
        let state: ProposalState = serde_json::from_slice(&session.mode_state).unwrap();
        assert_eq!(state.accepts.get("buyer"), Some(&"p2".to_string()));
    }

    #[test]
    fn withdrawn_proposal_cannot_be_accepted() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Withdraw", make_withdraw("p1", "withdrawn")),
            )
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env("buyer", "Accept", make_accept("p1", "too late")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Reject ---

    #[test]
    fn reject_existing_proposal() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("buyer", "Reject", make_reject("p1", false, "too expensive")),
            )
            .unwrap();
        assert!(matches!(result, ModeResponse::PersistState(_)));
    }

    #[test]
    fn terminal_reject_sets_flag() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("buyer", "Reject", make_reject("p1", true, "deal breaker")),
            )
            .unwrap();
        apply(&mut session, result);
        let state: ProposalState = serde_json::from_slice(&session.mode_state).unwrap();
        assert_eq!(state.terminal_rejections.len(), 1);
    }

    #[test]
    fn reject_nonexistent_proposal_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env("buyer", "Reject", make_reject("nope", false, "bad")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Withdraw ---

    #[test]
    fn withdraw_own_proposal() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Withdraw", make_withdraw("p1", "changed mind")),
            )
            .unwrap();
        apply(&mut session, result);
        let state: ProposalState = serde_json::from_slice(&session.mode_state).unwrap();
        assert_eq!(
            state.proposals["p1"].disposition,
            ProposalDisposition::Withdrawn
        );
    }

    #[test]
    fn cannot_withdraw_others_proposal() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env("buyer", "Withdraw", make_withdraw("p1", "nope")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");
    }

    #[test]
    fn withdraw_clears_acceptances() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(&session, &env("buyer", "Accept", make_accept("p1", "ok")))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Withdraw", make_withdraw("p1", "changed mind")),
            )
            .unwrap();
        apply(&mut session, result);
        let state: ProposalState = serde_json::from_slice(&session.mode_state).unwrap();
        assert!(state.accepts.is_empty());
    }

    #[test]
    fn double_withdraw_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Withdraw", make_withdraw("p1", "withdrawn")),
            )
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(
                &session,
                &env("seller", "Withdraw", make_withdraw("p1", "again")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Commitment ---

    #[test]
    fn commitment_resolves_session() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(&session, &env("buyer", "Commitment", commitment_payload()))
            .unwrap();
        assert!(matches!(result, ModeResponse::PersistAndResolve { .. }));
    }

    #[test]
    fn commitment_without_proposals_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(&session, &env("buyer", "Commitment", commitment_payload()))
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn non_initiator_commitment_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(&session, &env("seller", "Commitment", commitment_payload()))
            .unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");
    }

    // --- Authorization ---

    #[test]
    fn non_participant_rejected() {
        let mode = ProposalMode;
        let session = base_session();
        let err = mode
            .authorize_sender(
                &session,
                &env("outsider", "Proposal", make_proposal("p1", "Offer", "1200")),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Forbidden");
    }

    // --- Unknown message type ---

    #[test]
    fn unknown_message_type_rejected() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);
        let err = mode
            .on_message(&session, &env("seller", "CustomType", vec![]))
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    // --- Full lifecycle ---

    #[test]
    fn full_proposal_lifecycle() {
        let mode = ProposalMode;
        let mut session = base_session();
        let result = mode
            .on_session_start(&session, &env("buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, result);

        // Seller makes proposal
        let result = mode
            .on_message(
                &session,
                &env("seller", "Proposal", make_proposal("p1", "Initial", "1200")),
            )
            .unwrap();
        apply(&mut session, result);

        // Buyer counter-proposes
        let result = mode
            .on_message(
                &session,
                &env(
                    "buyer",
                    "CounterProposal",
                    make_counter("p2", "p1", "Counter", "1000"),
                ),
            )
            .unwrap();
        apply(&mut session, result);

        // Both accept p2
        let result = mode
            .on_message(&session, &env("buyer", "Accept", make_accept("p2", "good")))
            .unwrap();
        apply(&mut session, result);
        let result = mode
            .on_message(
                &session,
                &env("seller", "Accept", make_accept("p2", "agreed")),
            )
            .unwrap();
        apply(&mut session, result);

        // Commitment
        let result = mode
            .on_message(&session, &env("buyer", "Commitment", commitment_payload()))
            .unwrap();
        assert!(matches!(result, ModeResponse::PersistAndResolve { .. }));
    }

    #[test]
    fn deterministic_serialization() {
        let mut state1 = ProposalState::default();
        state1.proposals.insert(
            "z".into(),
            ProposalRecord {
                proposal_id: "z".into(),
                title: "z".into(),
                summary: "".into(),
                details: vec![],
                tags: vec![],
                proposer: "".into(),
                supersedes_proposal_id: None,
                disposition: ProposalDisposition::Live,
            },
        );
        state1.proposals.insert(
            "a".into(),
            ProposalRecord {
                proposal_id: "a".into(),
                title: "a".into(),
                summary: "".into(),
                details: vec![],
                tags: vec![],
                proposer: "".into(),
                supersedes_proposal_id: None,
                disposition: ProposalDisposition::Live,
            },
        );

        let mut state2 = ProposalState::default();
        state2.proposals.insert(
            "a".into(),
            ProposalRecord {
                proposal_id: "a".into(),
                title: "a".into(),
                summary: "".into(),
                details: vec![],
                tags: vec![],
                proposer: "".into(),
                supersedes_proposal_id: None,
                disposition: ProposalDisposition::Live,
            },
        );
        state2.proposals.insert(
            "z".into(),
            ProposalRecord {
                proposal_id: "z".into(),
                title: "z".into(),
                summary: "".into(),
                details: vec![],
                tags: vec![],
                proposer: "".into(),
                supersedes_proposal_id: None,
                disposition: ProposalDisposition::Live,
            },
        );

        assert_eq!(
            ProposalMode::encode_state(&state1),
            ProposalMode::encode_state(&state2)
        );
    }
}
