use crate::error::MacpError;
use crate::mode::util::{
    is_declared_participant, participants_all_accept, validate_commitment_payload_for_session,
};
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

    fn commitment_ready(session: &Session, state: &ProposalState) -> bool {
        if !state.terminal_rejections.is_empty() {
            return true;
        }

        state
            .proposals
            .values()
            .filter(|proposal| proposal.disposition == ProposalDisposition::Live)
            .any(|proposal| {
                participants_all_accept(
                    &session.participants,
                    &state.accepts,
                    &proposal.proposal_id,
                )
            })
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
                state.accepts.retain(|_, pid| pid != &payload.proposal_id);
                state
                    .terminal_rejections
                    .retain(|r| r.proposal_id != payload.proposal_id);
                Ok(ModeResponse::PersistState(Self::encode_state(&state)))
            }
            "Commitment" => {
                if env.sender != session.initiator_sender {
                    return Err(MacpError::Forbidden);
                }
                validate_commitment_payload_for_session(session, &env.payload)?;
                if !Self::commitment_ready(session, &state) {
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
            participants: vec!["agent://buyer".into(), "agent://seller".into()],
            seen_message_ids: HashSet::new(),
            intent: String::new(),
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            context: vec![],
            roots: vec![],
            initiator_sender: "agent://buyer".into(),
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

    fn commitment(session: &Session, action: &str) -> Vec<u8> {
        CommitmentPayload {
            commitment_id: "c1".into(),
            action: action.into(),
            authority_scope: "commercial".into(),
            reason: "bound".into(),
            mode_version: session.mode_version.clone(),
            policy_version: session.policy_version.clone(),
            configuration_version: session.configuration_version.clone(),
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

    fn make_proposal(id: &str) -> Vec<u8> {
        ProposalPayload {
            proposal_id: id.into(),
            title: format!("offer-{id}"),
            summary: "summary".into(),
            details: vec![],
            tags: vec![],
        }
        .encode_to_vec()
    }

    fn make_accept(id: &str) -> Vec<u8> {
        AcceptPayload {
            proposal_id: id.into(),
            reason: String::new(),
        }
        .encode_to_vec()
    }

    fn make_reject(id: &str, terminal: bool) -> Vec<u8> {
        RejectPayload {
            proposal_id: id.into(),
            terminal,
            reason: "no".into(),
        }
        .encode_to_vec()
    }

    fn make_withdraw(id: &str) -> Vec<u8> {
        WithdrawPayload {
            proposal_id: id.into(),
            reason: "changed mind".into(),
        }
        .encode_to_vec()
    }

    #[test]
    fn session_start_requires_participants() {
        let mode = ProposalMode;
        let mut session = base_session();
        session.participants.clear();
        assert_eq!(
            mode.on_session_start(&session, &env("agent://buyer", "SessionStart", vec![]))
                .unwrap_err()
                .to_string(),
            "InvalidPayload"
        );
    }

    #[test]
    fn commitment_requires_acceptance_convergence() {
        let mode = ProposalMode;
        let mut session = base_session();
        let resp = mode
            .on_session_start(&session, &env("agent://buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Proposal", make_proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);

        assert_eq!(
            mode.on_message(
                &session,
                &env(
                    "agent://buyer",
                    "Commitment",
                    commitment(&session, "proposal.accepted"),
                ),
            )
            .unwrap_err()
            .to_string(),
            "InvalidPayload"
        );

        let resp = mode
            .on_message(&session, &env("agent://buyer", "Accept", make_accept("p1")))
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Accept", make_accept("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        mode.on_message(
            &session,
            &env(
                "agent://buyer",
                "Commitment",
                commitment(&session, "proposal.accepted"),
            ),
        )
        .unwrap();
    }

    #[test]
    fn terminal_rejection_allows_negative_commitment() {
        let mode = ProposalMode;
        let mut session = base_session();
        let resp = mode
            .on_session_start(&session, &env("agent://buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Proposal", make_proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://buyer", "Reject", make_reject("p1", true)),
            )
            .unwrap();
        apply(&mut session, resp);
        mode.on_message(
            &session,
            &env(
                "agent://buyer",
                "Commitment",
                commitment(&session, "proposal.rejected"),
            ),
        )
        .unwrap();
    }

    #[test]
    fn withdraw_clears_terminal_rejections() {
        let mode = ProposalMode;
        let mut session = base_session();
        let resp = mode
            .on_session_start(&session, &env("agent://buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, resp);
        // Seller proposes p1
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Proposal", make_proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        // Buyer terminally rejects p1
        let resp = mode
            .on_message(
                &session,
                &env("agent://buyer", "Reject", make_reject("p1", true)),
            )
            .unwrap();
        apply(&mut session, resp);
        // Seller withdraws p1 — terminal rejection should be cleared
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Withdraw", make_withdraw("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        // Seller proposes p2
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Proposal", make_proposal("p2")),
            )
            .unwrap();
        apply(&mut session, resp);
        // Commitment should NOT be ready (no acceptance convergence, no terminal rejection)
        let err = mode
            .on_message(
                &session,
                &env(
                    "agent://buyer",
                    "Commitment",
                    commitment(&session, "proposal.rejected"),
                ),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "InvalidPayload");
    }

    #[test]
    fn terminal_rejection_on_different_proposal_survives_withdraw() {
        let mode = ProposalMode;
        let mut session = base_session();
        let resp = mode
            .on_session_start(&session, &env("agent://buyer", "SessionStart", vec![]))
            .unwrap();
        apply(&mut session, resp);
        // Seller proposes p1 and p2
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Proposal", make_proposal("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Proposal", make_proposal("p2")),
            )
            .unwrap();
        apply(&mut session, resp);
        // Buyer terminally rejects p2
        let resp = mode
            .on_message(
                &session,
                &env("agent://buyer", "Reject", make_reject("p2", true)),
            )
            .unwrap();
        apply(&mut session, resp);
        // Seller withdraws p1 — p2's terminal rejection survives
        let resp = mode
            .on_message(
                &session,
                &env("agent://seller", "Withdraw", make_withdraw("p1")),
            )
            .unwrap();
        apply(&mut session, resp);
        // Commitment should be ready because p2 still has a terminal rejection
        mode.on_message(
            &session,
            &env(
                "agent://buyer",
                "Commitment",
                commitment(&session, "proposal.rejected"),
            ),
        )
        .unwrap();
    }
}
