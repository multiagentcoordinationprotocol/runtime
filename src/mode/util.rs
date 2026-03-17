use crate::error::MacpError;
use crate::pb::CommitmentPayload;
use crate::session::Session;
use prost::Message;

pub fn decode_commitment_payload(payload: &[u8]) -> Result<CommitmentPayload, MacpError> {
    CommitmentPayload::decode(payload).map_err(|_| MacpError::InvalidPayload)
}

pub fn validate_commitment_payload_for_session(
    session: &Session,
    payload: &[u8],
) -> Result<CommitmentPayload, MacpError> {
    let commitment = decode_commitment_payload(payload)?;

    if commitment.commitment_id.trim().is_empty()
        || commitment.action.trim().is_empty()
        || commitment.authority_scope.trim().is_empty()
        || commitment.reason.trim().is_empty()
    {
        return Err(MacpError::InvalidPayload);
    }

    if commitment.mode_version != session.mode_version
        || commitment.configuration_version != session.configuration_version
    {
        return Err(MacpError::InvalidPayload);
    }

    if !session.policy_version.is_empty() && commitment.policy_version != session.policy_version {
        return Err(MacpError::InvalidPayload);
    }

    Ok(commitment)
}

pub fn is_declared_participant(participants: &[String], sender: &str) -> bool {
    participants.iter().any(|participant| participant == sender)
}

pub fn participants_all_accept(
    participants: &[String],
    accepts: &std::collections::BTreeMap<String, String>,
    proposal_id: &str,
) -> bool {
    !participants.is_empty()
        && participants
            .iter()
            .all(|participant| accepts.get(participant).map(String::as_str) == Some(proposal_id))
}
