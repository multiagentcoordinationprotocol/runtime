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

    // Validate outcome_positive consistency with action (RFC-0001 §7.3)
    validate_outcome_positive(&commitment)?;

    Ok(commitment)
}

/// Validate that `outcome_positive` is consistent with the `action` field.
/// Actions ending in `rejected`, `failed`, or `declined` must have `outcome_positive = false`.
/// Actions ending in `selected`, `accepted`, `completed`, or `approved` must have `outcome_positive = true`.
fn validate_outcome_positive(commitment: &CommitmentPayload) -> Result<(), MacpError> {
    let action = commitment.action.as_str();
    let negative_actions = ["rejected", "failed", "declined"];
    let positive_actions = ["selected", "accepted", "completed", "approved"];

    let is_negative = negative_actions
        .iter()
        .any(|suffix| action.ends_with(suffix));
    let is_positive = positive_actions
        .iter()
        .any(|suffix| action.ends_with(suffix));

    if is_negative && commitment.outcome_positive {
        return Err(MacpError::InvalidPayload);
    }
    if is_positive && !commitment.outcome_positive {
        return Err(MacpError::InvalidPayload);
    }
    Ok(())
}

pub fn is_declared_participant(participants: &[String], sender: &str) -> bool {
    participants.iter().any(|participant| participant == sender)
}

/// Check whether the sender is authorized to commit per the policy's `commitment.authority` rule.
///
/// RFC-MACP-0012 §4: the `commitment` rule group controls who can emit a Commitment
/// envelope. If no policy is bound, defaults to initiator-only (RFC-MACP-0001 §7.3).
pub fn check_commitment_authority(session: &Session, sender: &str) -> Result<(), MacpError> {
    if let Some(ref policy) = session.policy_definition {
        let rules: crate::policy::rules::CommitmentRules = extract_commitment_rules(&policy.rules);
        match rules.authority.as_str() {
            "any_participant" => {
                if sender == session.initiator_sender
                    || is_declared_participant(&session.participants, sender)
                {
                    Ok(())
                } else {
                    Err(MacpError::Forbidden)
                }
            }
            "designated_role" => {
                if rules.designated_roles.iter().any(|r| r == sender) {
                    Ok(())
                } else {
                    Err(MacpError::Forbidden)
                }
            }
            _ => {
                // "initiator_only" (default)
                if sender == session.initiator_sender {
                    Ok(())
                } else {
                    Err(MacpError::Forbidden)
                }
            }
        }
    } else {
        // No policy bound — default to initiator-only
        if sender == session.initiator_sender {
            Ok(())
        } else {
            Err(MacpError::Forbidden)
        }
    }
}

/// Extract the `commitment` section from any mode's policy rules JSON.
/// All RFC mode schemas include a `commitment` sub-object with `authority` and `designated_roles`.
fn extract_commitment_rules(rules: &serde_json::Value) -> crate::policy::rules::CommitmentRules {
    rules
        .get("commitment")
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default()
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
