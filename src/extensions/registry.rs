use super::provider::{SessionExtensionProvider, SessionOutcome};
use std::collections::HashMap;

pub struct ExtensionProviderRegistry {
    providers: Vec<Box<dyn SessionExtensionProvider>>,
}

impl ExtensionProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn SessionExtensionProvider>) {
        tracing::info!(key = provider.key(), "registered extension provider");
        self.providers.push(provider);
    }

    pub async fn on_session_start(&self, session_id: &str, extensions: &HashMap<String, Vec<u8>>) {
        for provider in &self.providers {
            if !extensions.contains_key(provider.key()) {
                continue;
            }
            if let Err(e) = provider.on_session_start(session_id, extensions).await {
                tracing::warn!(
                    key = provider.key(),
                    session_id,
                    error = %e,
                    "extension provider on_session_start failed (non-fatal)"
                );
            }
        }
    }

    pub async fn on_session_terminal(&self, session_id: &str, outcome: SessionOutcome) {
        for provider in &self.providers {
            if let Err(e) = provider
                .on_session_terminal(session_id, outcome_ref(&outcome))
                .await
            {
                tracing::warn!(
                    key = provider.key(),
                    session_id,
                    error = %e,
                    "extension provider on_session_terminal failed (non-fatal)"
                );
            }
        }
    }
}

impl Default for ExtensionProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn outcome_ref(outcome: &SessionOutcome) -> SessionOutcome {
    match outcome {
        SessionOutcome::Resolved => SessionOutcome::Resolved,
        SessionOutcome::Expired => SessionOutcome::Expired,
    }
}
