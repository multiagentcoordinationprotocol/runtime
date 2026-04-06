pub mod defaults;
pub mod evaluator;
pub mod registry;
pub mod rules;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyDefinition {
    pub policy_id: String,
    pub mode: String,
    pub description: String,
    pub rules: serde_json::Value,
    pub schema_version: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PolicyDecision {
    Allow { reasons: Vec<String> },
    Deny { reasons: Vec<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PolicyError {
    UnknownPolicy(String),
    InvalidDefinition(String),
    PolicyDenied(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::UnknownPolicy(id) => write!(f, "unknown policy: {}", id),
            PolicyError::InvalidDefinition(msg) => write!(f, "invalid policy definition: {}", msg),
            PolicyError::PolicyDenied(reason) => write!(f, "policy denied: {}", reason),
        }
    }
}

impl std::error::Error for PolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_error_display() {
        let e = PolicyError::UnknownPolicy("p1".into());
        assert_eq!(e.to_string(), "unknown policy: p1");

        let e = PolicyError::InvalidDefinition("bad".into());
        assert_eq!(e.to_string(), "invalid policy definition: bad");

        let e = PolicyError::PolicyDenied("nope".into());
        assert_eq!(e.to_string(), "policy denied: nope");
    }

    #[test]
    fn policy_definition_serialization_round_trip() {
        let def = PolicyDefinition {
            policy_id: "test".into(),
            mode: "*".into(),
            description: "test policy".into(),
            rules: serde_json::json!({"voting": {"algorithm": "none"}}),
            schema_version: 1,
        };
        let json = serde_json::to_string(&def).unwrap();
        let parsed: PolicyDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.policy_id, "test");
        assert_eq!(parsed.schema_version, 1);
    }
}
