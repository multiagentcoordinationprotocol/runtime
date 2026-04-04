use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

use crate::helpers;
use super::SharedClient;
use super::decision::MacpToolError;

#[derive(Clone)]
pub struct CommitTool {
    pub client: SharedClient,
    pub agent_id: String,
    pub mode: String,
}

#[derive(Deserialize)]
pub struct CommitArgs {
    pub session_id: String,
    pub action: String,
    pub authority_scope: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct CommitResult {
    pub ok: bool,
    pub session_state: i32,
}

impl Tool for CommitTool {
    const NAME: &'static str = "macp_commit";
    type Error = MacpToolError;
    type Args = CommitArgs;
    type Output = CommitResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_commit",
            "description": "Commit and resolve a MACP session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "action": { "type": "string", "description": "The resolved action" },
                    "authority_scope": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["session_id", "action", "authority_scope", "reason"]
            }
        })).unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::commitment_payload(
            &helpers::new_message_id(),
            &args.action,
            &args.authority_scope,
            &args.reason,
        );
        let env = helpers::envelope(
            &self.mode, "Commitment",
            &helpers::new_message_id(), &args.session_id, &self.agent_id, payload,
        );
        let mut client = self.client.lock().await;
        let ack = helpers::send_as(&mut client, &self.agent_id, env)
            .await.map_err(|e| MacpToolError(e.to_string()))?;
        Ok(CommitResult { ok: ack.ok, session_state: ack.session_state })
    }
}
