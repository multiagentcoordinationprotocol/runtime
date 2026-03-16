use crate::error::MacpError;
use crate::pb::CommitmentPayload;
use prost::Message;

pub fn decode_commitment_payload(payload: &[u8]) -> Result<CommitmentPayload, MacpError> {
    CommitmentPayload::decode(payload).map_err(|_| MacpError::InvalidPayload)
}

pub fn is_declared_participant(participants: &[String], sender: &str) -> bool {
    participants.iter().any(|participant| participant == sender)
}
