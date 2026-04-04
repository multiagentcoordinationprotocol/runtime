use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

use crate::helpers;
use super::SharedClient;
use super::decision::MacpToolError;

#[derive(Serialize)]
pub struct ToolResult {
    pub ok: bool,
    pub session_state: i32,
}

#[derive(Clone)]
pub struct HandoffOfferTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct HandoffOfferArgs {
    pub session_id: String,
    pub handoff_id: String,
    pub target: String,
    pub scope: String,
    pub reason: String,
}

impl Tool for HandoffOfferTool {
    const NAME: &'static str = "macp_handoff_offer";
    type Error = MacpToolError;
    type Args = HandoffOfferArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_handoff_offer",
            "description": "Offer a handoff to another agent in a MACP Handoff session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "handoff_id": { "type": "string" },
                    "target": { "type": "string" },
                    "scope": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["session_id", "handoff_id", "target", "scope", "reason"]
            }
        })).unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::handoff_offer_payload(&args.handoff_id, &args.target, &args.scope, &args.reason);
        let env = helpers::envelope(
            helpers::MODE_HANDOFF, "HandoffOffer",
            &helpers::new_message_id(), &args.session_id, &self.agent_id, payload,
        );
        let mut client = self.client.lock().await;
        let ack = helpers::send_as(&mut client, &self.agent_id, env)
            .await.map_err(|e| MacpToolError(e.to_string()))?;
        Ok(ToolResult { ok: ack.ok, session_state: ack.session_state })
    }
}

#[derive(Clone)]
pub struct HandoffAcceptTool {
    pub client: SharedClient,
    pub agent_id: String,
}

#[derive(Deserialize)]
pub struct HandoffAcceptArgs {
    pub session_id: String,
    pub handoff_id: String,
    pub reason: String,
}

impl Tool for HandoffAcceptTool {
    const NAME: &'static str = "macp_handoff_accept";
    type Error = MacpToolError;
    type Args = HandoffAcceptArgs;
    type Output = ToolResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "macp_handoff_accept",
            "description": "Accept a handoff in a MACP Handoff session",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "handoff_id": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["session_id", "handoff_id", "reason"]
            }
        })).unwrap()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let payload = helpers::handoff_accept_payload(&args.handoff_id, &self.agent_id, &args.reason);
        let env = helpers::envelope(
            helpers::MODE_HANDOFF, "HandoffAccept",
            &helpers::new_message_id(), &args.session_id, &self.agent_id, payload,
        );
        let mut client = self.client.lock().await;
        let ack = helpers::send_as(&mut client, &self.agent_id, env)
            .await.map_err(|e| MacpToolError(e.to_string()))?;
        Ok(ToolResult { ok: ack.ok, session_state: ack.session_state })
    }
}
