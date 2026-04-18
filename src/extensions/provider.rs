use std::collections::HashMap;

#[derive(Debug)]
pub enum ExtensionError {
    Internal(String),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtensionError::Internal(msg) => write!(f, "extension error: {msg}"),
        }
    }
}

impl std::error::Error for ExtensionError {}

pub enum SessionOutcome {
    Resolved,
    Expired,
}

/// Trait for pluggable session extension providers.
///
/// Providers receive lifecycle callbacks for sessions that carry their
/// declared key in the `extensions` map. Provider errors are never fatal
/// to session lifecycle (invariant E-1).
#[async_trait::async_trait]
pub trait SessionExtensionProvider: Send + Sync {
    fn key(&self) -> &str;

    async fn on_session_start(
        &self,
        session_id: &str,
        extensions: &HashMap<String, Vec<u8>>,
    ) -> Result<(), ExtensionError>;

    async fn on_session_terminal(
        &self,
        session_id: &str,
        outcome: SessionOutcome,
    ) -> Result<(), ExtensionError>;
}
